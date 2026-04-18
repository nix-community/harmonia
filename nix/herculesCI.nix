{
  self,
  lib,
  systems,
  pkgs,
}:
# buildbot-effects compatible hercules-ci effects
# buildbot-effects passes: { branch, rev, shortRev, name, tag, remoteHttpUrl, primaryRepo }
args:
let
  # Only run effect on main branch
  isMain = args.branch == "main";

  # Skip x86_64-darwin for now - rustc builds are slow and often not cached
  uploadSystems = builtins.filter (s: s != "x86_64-darwin") systems;

  # codecov-cli depends on test-results-parser which has an unfree license
  # Override just the license instead of using allowUnfree (expensive)
  codecov-cli = pkgs.codecov-cli.override {
    python3Packages = pkgs.python3Packages.overrideScope (
      _final: prev: {
        test-results-parser = prev.test-results-parser.overrideAttrs {
          meta.license = lib.licenses.free;
        };
      }
    );
  };

  # Get test outputs for all systems (list -> attrset)
  testOutputs = lib.genAttrs uploadSystems (system: self.checks.${system}.tests);
in
{
  onPush.default.outputs.effects.uploadCodecov.run =
    if isMain then
      pkgs.stdenv.mkDerivation {
        name = "upload-codecov-effect";
        # Only run buildPhase - skip unpack/install which don't work in effects
        phases = [ "buildPhase" ];
        nativeBuildInputs = [
          pkgs.git
          pkgs.jq
          codecov-cli
        ];
        # Run from source directory so codecov-cli can discover network files via git
        buildPhase = ''
          set -euo pipefail

          # Set HOME for codecov-cli (runs in sandbox without it)
          export HOME=/tmp

          # Copy source and initialize git repo for network file discovery
          # (self doesn't include .git directory)
          cp -r ${self} src
          chmod -R u+w src
          cd src
          git init
          git add .

          # Read codecov token from secrets
          CODECOV_TOKEN=$(jq -r '.codecov.data.token // empty' "$HERCULES_CI_SECRETS_JSON")

          if [[ -z "$CODECOV_TOKEN" ]]; then
            echo "ERROR: Codecov token not found in secrets"
            echo "Expected structure: { \"codecov\": { \"data\": { \"token\": \"...\" } } }"
            exit 1
          fi

          # Create commit in codecov first
          echo "Creating commit in codecov..."
          codecovcli create-commit \
            --token "$CODECOV_TOKEN" \
            --slug "nix-community/harmonia" \
            --git-service github \
            --sha "${args.rev}" \
            --branch "${args.branch}"

          # Create report for the commit
          echo "Creating report in codecov..."
          codecovcli create-report \
            --token "$CODECOV_TOKEN" \
            --slug "nix-community/harmonia" \
            --git-service github \
            --sha "${args.rev}"

          # Upload coverage for each system
          # --disable-search: don't auto-search for coverage files, we specify them explicitly
          # Network files are still discovered via git ls-files since we're in the source directory
          ${lib.concatMapStringsSep "\n" (system: ''
            echo "Uploading coverage for ${system}..."
            if [[ -f "${testOutputs.${system}}/${system}.json" ]]; then
              codecovcli do-upload \
                --token "$CODECOV_TOKEN" \
                --slug "nix-community/harmonia" \
                --git-service github \
                --file "${testOutputs.${system}}/${system}.json" \
                --sha "${args.rev}" \
                --branch "${args.branch}" \
                --flag "${system}"
            else
              echo "No coverage file found for ${system}"
            fi
          '') uploadSystems}
        '';
      }
    else
      null;
}
