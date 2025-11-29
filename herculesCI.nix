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
          # Get test outputs for all systems
          testOutputs = builtins.mapAttrs (
            system: _: withSystem system ({ self', ... }: self'.checks.tests)
          ) config.systems;
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
                          ${pkgs.codecov-cli}/bin/codecovcli do-upload \
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
                      '') (builtins.attrNames config.systems)
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
