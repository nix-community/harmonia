# harmonia-store-ref-scan

Streaming reference scanner for Nix store path outputs.

## Overview

After a build completes, Nix needs to discover which store paths the
output references. `RefScanSink` is a streaming scanner that can be
fed arbitrary byte chunks (typically a NAR stream) and efficiently
finds embedded store-path hash references using the same Boyer-Moore
style window scan as Nix's `references.cc`.
