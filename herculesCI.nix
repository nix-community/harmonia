{
  config,
  withSystem,
  ...
}:
{
  # buildbot-effects compatible hercules-ci effects
  # buildbot-effects passes: { branch, rev, shortRev, name, tag, remoteHttpUrl, primaryRepo }
  # Use flake.herculesCI to define the output directly (not through hercules-ci-effects module)
  flake.herculesCI =
    args:
    let
      # Only run effect on main branch
      isMain = args.branch == "main";
    in
    {
      onPush.default.outputs.effects = withSystem "x86_64-linux" (
        { pkgs, ... }:
        let
          inherit (pkgs) lib;
          # config.systems is a list in flake-parts
          systems = config.systems;

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
          testOutputs = builtins.listToAttrs (
            builtins.map (system: {
              name = system;
              value = withSystem system ({ self', ... }: self'.checks.tests);
            }) systems
          );
        in
        {
          uploadCodecov = {
            # buildbot-effects instantiates .run and runs the derivation's builder
            # The effect code runs as the build script
            run =
              if isMain then
                pkgs.stdenv.mkDerivation {
                  name = "upload-codecov-effect";
                  # Effect derivations don't need sources
                  dontUnpack = true;
                  # The build phase IS the effect - it runs when buildbot-effects executes this
                  buildPhase = ''
                    set -euo pipefail

                    # Read codecov token from secrets
                    CODECOV_TOKEN=$(${pkgs.jq}/bin/jq -r '.codecov.data.token // empty' "$HERCULES_CI_SECRETS_JSON")

                    if [[ -z "$CODECOV_TOKEN" ]]; then
                      echo "ERROR: Codecov token not found in secrets"
                      echo "Expected structure: { \"codecov\": { \"data\": { \"token\": \"...\" } } }"
                      exit 1
                    fi

                    # Upload coverage for each system
                    ${builtins.concatStringsSep "\n" (
                      builtins.map (system: ''
                        echo "Uploading coverage for ${system}..."
                        if [[ -f "${testOutputs.${system}}/${system}.json" ]]; then
                          ${codecov-cli}/bin/codecovcli do-upload \
                            --token "$CODECOV_TOKEN" \
                            --slug "nix-community/harmonia" \
                            --git-service github \
                            --file "${testOutputs.${system}}/${system}.json" \
                            --disable-search \
                            --sha "${args.rev}" \
                            --branch "${args.branch}" \
                            --flag "${system}"
                        else
                          echo "No coverage file found for ${system}"
                        fi
                      '') systems
                    )}
                  '';
                  installPhase = "touch $out";
                }
              else
                null;
          };
        }
      );
    };
}
