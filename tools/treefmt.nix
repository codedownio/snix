# Builds treefmt for depot, with a hardcoded configuration that
# includes the right paths to formatters.
{ pkgs, lib, ... }:

let
  treefmt = pkgs.treefmt.withConfig {
    name = "depot-treefmt";

    settings = {
      on-unmatched = "debug";

      formatter = {
        go = {
          command = lib.getExe' pkgs.go "gofmt";
          options = [ "-w" ];
          includes = [ "*.go" ];
        };

        nix = {
          command = lib.getExe pkgs.nixfmt;
          includes = [ "*.nix" ];
          excludes = [
            "snix/eval/src/tests/nix_tests/*"
            "snix/eval/src/tests/snix_tests/*"
            "snix/glue/src/tests/nix_tests/*"
            "snix/glue/src/tests/snix_tests/*"
            "contrib/nix-language-test-suite/tests/*"
          ];
        };

        rust = {
          command = lib.getExe pkgs.rustfmt;
          includes = [ "*.rs" ];
        };

        toml = {
          command = lib.getExe pkgs.taplo;
          options = [ "format" ];
          includes = [ "*.toml" ];
        };

        editorconfig = {
          command = lib.getExe pkgs.editorconfig-checker;
          includes = [
            "*.c"
            "*.conf"
            "*.css"
            "*.exp"
            "*.go"
            "*.h"
            "*.hcl"
            "*.html"
            "*.java"
            "*.jq"
            "*.js"
            "*.json"
            "*.md"
            "*.nix"
            "*.proto"
            "*.py"
            "*.rs"
            "*.scm"
            "*.scss"
            "*.sh"
            "*.tf"
            "*.toml"
            "*.txt"
            "*.xml"
            "*.yaml"
            "*.yml"
          ];
          excludes = [
            "snix/eval/src/tests/nix_tests/*"
            "snix/glue/src/tests/nix_tests/*"
            "contrib/nix-language-test-suite/tests/*"
          ];
        };
      };
    };
  };

  # wrapper script for running formatting checks in CI. treefmt finds the tree
  # root itself via `git rev-parse`, so no --tree-root is needed.
  check = pkgs.writeShellScript "treefmt-check" ''
    ${treefmt}/bin/treefmt \
      --no-cache \
      --fail-on-change
  '';
in
# treefmt is invoked directly (it resolves the tree root via `git rev-parse`,
# so it works from any subdirectory), with the CI check attached as passthru.
treefmt.overrideAttrs (prev: {
  passthru = (prev.passthru or { }) // {
    inherit check;
  };
  meta = (prev.meta or { }) // {
    ci.extraSteps.check = {
      label = "depot formatting check";
      command = check;
      alwaysRun = true;
    };
    ci.fast = true;
  };
})
