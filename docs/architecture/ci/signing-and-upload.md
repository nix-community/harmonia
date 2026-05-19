# Signing and Upload

## Presigned S3 URLs

This is the key architectural choice that removes the binary cache as a
bottleneck. Builders upload build outputs **directly to S3** using
short-lived presigned PUT URLs. The signer node signs the URL but never
sees the NAR bytes.

### The constraint: key must be known before signing

An S3 presigned PUT URL bakes in: bucket, object key, expiry, and
optionally Content-Type. The object key for a NAR is the NAR hash
(`nar/<narhash>.nar.zst`), which is not known until after the build
completes and the output NAR is computed.

Therefore the upload flow is:

```
1. Builder binary requests build via harmonia-daemon connection
   (our builder process drives the harmonia-daemon directly,
    no post-build-hook — we own the full build lifecycle)
         │
         ▼
2. Build completes; builder transitions job status: building → uploading
   Builder computes for each output:
     - streams /nix/store/<out> to tmpfs as zstd-compressed NAR
     - computes sha256(NAR) and sha256(compressed NAR) in one pass
         │
         ▼
3. POST /api/upload-slots  (to any signer-capable node)
   body: {
     outputs: [{
       store_path:       "/nix/store/abc...-foo",
       nar_hash:         "sha256:...",    # sha256 of uncompressed NAR
       nar_size:         12345678,
       compressed_hash:  "sha256:...",    # sha256 of zstd NAR (S3 object hash)
       compressed_size:  4567890,
       references:       ["xyz...-bar"],
       deriver:          "def...-foo.drv"
     }],
     log_compressed_size: 98765,          # zstd-compressed log size
     log_key:            "log/abc...-foo.drv"
   }
         │
         ▼
4. Signer node, for each output:
   - checks if NAR already in S3 (HEAD s3://bucket/nar/<narhash>.nar.zst)
   - if present: marks output as skipped (already uploaded by another node)
   - if absent:
       generates presigned PUT URL for NAR:
         key     = "nar/<narhash>.nar.zst"
         expiry  = 15 minutes
         headers = { Content-Type: application/zstd,
                     Content-Length: <compressed_size>,
                     x-amz-checksum-sha256: <compressed_hash_b64> }
   - generates presigned PUT URL for build log:
       key     = log_key (e.g. "log/abc...-foo.drv")
       expiry  = 15 minutes
   - returns: { nar_urls: [{presigned_url, skipped}], log_url: presigned_url }
   (narinfo is NOT returned here — signer writes it directly in step 7)
         │
         ▼
5. Builder:
   - PUT each non-skipped NAR to S3 via presigned URL
   - PUT zstd-compressed build log to S3 via presigned URL
   (signer node not in the data path for any of these)
         │
         ▼
6. Builder: POST /api/build-complete  (to any signer-capable node)
   body: { job_id, outputs: [{ store_path, nar_hash, nar_size,
            compressed_hash, compressed_size, references }] }
         │
         ▼
7. Signer node:
   - signs each narinfo (Ed25519 fingerprint: 1;<store_path>;<nar_hash>;<nar_size>;<refs>)
   - writes each narinfo to S3: "<hash>.narinfo"
     (signer has direct S3 write access — not presigned — because it
      holds S3 credentials; builders only get presigned URLs)
   - updates canonical hashes: UPDATE derivation_outputs SET nar_hash=…
     WHERE nar_hash IS NULL (first writer wins)
   - UPDATE build_jobs SET status='succeeded', finished_at=now(),
     log_url='log/…'
   - NOTIFY PostgreSQL → frontend reports status to forge
```

## Why only signer nodes hold the key

The Nix binary cache signing key (`SecretKey` / Ed25519) must not be on
builder nodes — they are the less-trusted machines that run arbitrary
derivations. The signer verifies metadata reported by the builder, then
signs the narinfo fingerprint (`1;<store_path>;<nar_hash>;<nar_size>;<refs>`).
A builder cannot forge a valid narinfo without the key.

In small deployments the signer and builder capability can coexist on the
same node. Key isolation is a deployment choice, not an architectural one.

## Signer Failover

Builders discover signers at runtime via the `nodes` table. If a signer
request fails (connection refused, timeout), the builder picks a
different live signer and retries:

```
signers = SELECT endpoint FROM nodes
          WHERE 'signer' = ANY(capabilities)
            AND last_seen > now() - interval '2 minutes';

POST /api/upload-slots → signer A
  → connection refused?
  → retry with signer B (round-robin remaining signers)
  → all signers down? back off (exponential, 1s–30s), re-query nodes table
  → still down after 5 minutes? mark build as failed (transient)
```

**Crash recovery**: If a signer crashes after writing narinfo to S3 but
before updating `build_jobs` in PostgreSQL, the build remains in
`uploading` state. The heartbeat reaper eventually resets it to `pending`,
causing a rebuild. This is harmless: the signer's HEAD check on the next
attempt finds the NARs (and narinfo) already in S3 and skips them. The
only cost is a redundant build — acceptable for a rare edge case.

Multiple signer nodes can run simultaneously — they all hold the same
signing key and S3 credentials. The signer endpoint is stateless (no
session affinity needed).

The S3 bucket serves both NARs and narinfo files. Harmonia-cache in front
of S3 can serve them directly, or Nix clients can be pointed at S3
directly with `substituters = s3://bucket?scheme=https&endpoint=...`.

## Builder Trust Boundary

The signer trusts builder-reported `nar_hash`, `nar_size`, and
`references` without independent verification. Verifying these would
require the signer to download and decompress the NAR — defeating the
architecture where the signer never touches NAR bytes.

A compromised builder can therefore cause the signer to produce a
narinfo with incorrect references (broken closures) or incorrect
`nar_hash` (the uncompressed hash; S3 only verifies the compressed
hash). Defense against compromised builders requires multi-build
reproducibility verification, which is deferred to a later phase.

The Nix sandbox (`sandbox = true`) is the primary defense against
build output tampering on non-compromised builders.

## S3 Content Integrity

The presigned URL includes `x-amz-checksum-sha256`, which S3 verifies
server-side. If the builder uploads corrupt data, S3 rejects the PUT.
The signer never needs to re-verify.

## Presigned URL Expiry

Presigned URLs expire after 15 minutes. For very large outputs (multi-GB
NARs) on slow links, the upload may exceed this window. If S3 rejects
the PUT with 403 Forbidden, the builder re-requests presigned URLs from
the signer and retries. The signer's HEAD check skips outputs already
uploaded, so only the failed output is re-attempted. This retry is
bounded by the build timeout (4h) — if uploads keep failing, the job
eventually times out and is marked failed.

## S3 Object Layout

```
<hash>.narinfo              ← signed narinfo (top-level)
nar/<hash>.nar.zst          ← zstd-compressed NAR data
log/<drvname>.drv           ← build logs (zstd-compressed content)
<hash>.ls                   ← directory listings
realisations/<hash>!<out>.doi ← CA derivation realisations
```

This matches the layout used by [niks3](https://github.com/numtide/niks3).

## Compression

NAR data is stored in S3 as `zstd`-compressed objects
(`Content-Type: application/zstd`). The narinfo `Compression: zstd` and
`URL: nar/<narhash>.nar.zst` fields tell Nix clients to decompress on
download.

Workers compress during the post-build hash pass — one streaming pass
over the NAR produces both the uncompressed hash (for the narinfo
`NarHash`) and the compressed bytes (for S3 upload). No temporary
uncompressed copy is needed.

Compression ratios for typical Nix store content:
- Build outputs (ELF binaries, debug info): 2–4×
- Text paths (scripts, docs, man pages): 4–10×
- Source tarballs (already compressed): ~1× — stored as-is or recompressed
