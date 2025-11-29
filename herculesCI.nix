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
          # Skip x86_64-darwin for now - rustc builds are slow and often not cached
          systems = builtins.filter (s: s != "x86_64-darwin") config.systems;

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
            run =
              if isMain then
                pkgs.writeShellApplication {
                  name = "upload-codecov-effect";
                  runtimeInputs = [
                    pkgs.git
                    pkgs.jq
                    codecov-cli
                  ];
                  text = ''
                    # Set HOME for codecov-cli (runs in sandbox without it)
                    export HOME=/tmp

                    # Read codecov token from secrets
                    CODECOV_TOKEN=$(jq -r '.codecov.data.token // empty' "$HERCULES_CI_SECRETS_JSON")

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
                          codecovcli do-upload \
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
                }
              else
                null;
          };
        }
      );
    };
}
