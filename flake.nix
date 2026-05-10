{
  description = "Prototype durable terminal session owner with transcript replay.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        toolchain = fenix.packages.${system}.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-gh/xTkxKHL4eiRXzWv8KP7vfjSk61Iq48x47BEDFgfk=";
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        src = craneLib.cleanCargoSource ./.;
        commonArgs = {
          inherit src;
          strictDeps = true;
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in
      {
        packages.default = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
          }
        );

        checks = {
          default = craneLib.cargoBuild (commonArgs // { inherit cargoArtifacts; });
          build = craneLib.cargoBuild (commonArgs // { inherit cargoArtifacts; });
          fmt = craneLib.cargoFmt { inherit src; };
          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );
        };

        apps.session-witnesses = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-lab-session-witnesses";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test session_witnesses -- --nocapture
            '';
          };
        };

        apps.agent-terminal-witness = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-lab-agent-terminal-witness";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test agent_terminal_witness -- --nocapture
            '';
          };
        };

        apps.daemon-witness = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-lab-daemon-witness";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test daemon_witness -- --nocapture
            '';
          };
        };

        apps.ghostty-agent-demo = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-lab-ghostty-agent-demo";
            runtimeInputs = [
              pkgs.coreutils
              toolchain
            ];
            text = ''
              cargo build \
                --bin agent-terminal-fixture \
                --bin terminal-cell-lab-daemon \
                --bin terminal-cell-lab-view
              target_debug="$(pwd)/target/debug"
              root="$(mktemp -d -t terminal-cell-lab-ghostty.XXXXXX)"
              socket="$root/cell.sock"
              ready="$root/daemon.ready"
              view_ready="$root/view.ready"
              mkfifo "$ready"
              mkfifo "$view_ready"
              "$target_debug/terminal-cell-lab-daemon" \
                --socket "$socket" \
                -- "$target_debug/agent-terminal-fixture" > "$ready" &
              daemon_pid="$!"
              ghostty_pid=""
              trap 'if [ -n "$ghostty_pid" ]; then kill "$ghostty_pid" 2>/dev/null || true; wait "$ghostty_pid" 2>/dev/null || true; fi; kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; rm -rf "$root"' EXIT
              ready_line="$(timeout 20s head -n 1 "$ready")"
              if [ -z "$ready_line" ]; then
                printf 'terminal-cell-lab daemon did not announce readiness\n' >&2
                exit 1
              fi
              printf '%s\n' "$ready_line"

              ghostty_bin="''${GHOSTTY:-ghostty}"
              if ! command -v "$ghostty_bin" >/dev/null 2>&1; then
                if [ -x "$HOME/.nix-profile/bin/ghostty" ]; then
                  ghostty_bin="$HOME/.nix-profile/bin/ghostty"
                else
                  printf 'ghostty not found; set GHOSTTY=/path/to/ghostty\n' >&2
                  exit 1
                fi
              fi
              ghostty_class="''${TERMINAL_CELL_GHOSTTY_CLASS:-com.ligoldragon.terminalcellwitness}"

              "$ghostty_bin" --class="$ghostty_class" -e "$target_debug/terminal-cell-lab-view" --socket "$socket" --ready-file "$view_ready" &
              ghostty_pid="$!"
              view_line="$(timeout 20s head -n 1 "$view_ready")"
              if [ -z "$view_line" ]; then
                printf 'terminal-cell-lab view did not announce attachment\n' >&2
                exit 1
              fi
              printf '%s\n' "$view_line"
              wait "$ghostty_pid"
              ghostty_pid=""
            '';
          };
        };

        apps.ghostty-agent-witness = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-lab-ghostty-agent-witness";
            runtimeInputs = [
              pkgs.coreutils
              pkgs.gnugrep
              toolchain
            ];
            text = ''
              cargo build \
                --bin agent-terminal-fixture \
                --bin terminal-cell-lab-capture \
                --bin terminal-cell-lab-daemon \
                --bin terminal-cell-lab-send \
                --bin terminal-cell-lab-view \
                --bin terminal-cell-lab-wait
              target_debug="$(pwd)/target/debug"
              root="$(mktemp -d -t terminal-cell-lab-ghostty-witness.XXXXXX)"
              socket="$root/cell.sock"
              daemon_ready="$root/daemon.ready"
              view_ready="$root/view.ready"
              artifact_dir="target/ghostty-agent-witness"
              artifact="$artifact_dir/transcript.txt"
              mkdir -p "$artifact_dir"
              mkfifo "$daemon_ready"
              mkfifo "$view_ready"
              "$target_debug/terminal-cell-lab-daemon" \
                --socket "$socket" \
                -- "$target_debug/agent-terminal-fixture" > "$daemon_ready" &
              daemon_pid="$!"
              ghostty_pid=""
              trap 'if [ -n "$ghostty_pid" ]; then kill "$ghostty_pid" 2>/dev/null || true; wait "$ghostty_pid" 2>/dev/null || true; fi; kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; rm -rf "$root"' EXIT
              ready_line="$(timeout 20s head -n 1 "$daemon_ready")"
              if [ -z "$ready_line" ]; then
                printf 'terminal-cell-lab daemon did not announce readiness\n' >&2
                exit 1
              fi
              printf '%s\n' "$ready_line"

              ghostty_bin="''${GHOSTTY:-ghostty}"
              if ! command -v "$ghostty_bin" >/dev/null 2>&1; then
                if [ -x "$HOME/.nix-profile/bin/ghostty" ]; then
                  ghostty_bin="$HOME/.nix-profile/bin/ghostty"
                else
                  printf 'ghostty not found; set GHOSTTY=/path/to/ghostty\n' >&2
                  exit 1
                fi
              fi
              ghostty_class="''${TERMINAL_CELL_GHOSTTY_CLASS:-com.ligoldragon.terminalcellwitness}"

              "$ghostty_bin" --class="$ghostty_class" -e "$target_debug/terminal-cell-lab-view" --socket "$socket" --ready-file "$view_ready" &
              ghostty_pid="$!"
              view_line="$(timeout 20s head -n 1 "$view_ready")"
              if [ -z "$view_line" ]; then
                printf 'terminal-cell-lab view did not announce attachment\n' >&2
                exit 1
              fi
              printf '%s\n' "$view_line"

              "$target_debug/terminal-cell-lab-wait" --socket "$socket" --text agent-ready
              "$target_debug/terminal-cell-lab-send" --socket "$socket" --line "hello ghostty attach"
              "$target_debug/terminal-cell-lab-wait" --socket "$socket" --text "agent-response: hello ghostty attach"
              "$target_debug/terminal-cell-lab-capture" --socket "$socket" > "$artifact"
              grep -q "agent-response: hello ghostty attach" "$artifact"
              printf 'ghostty witness transcript=%s\n' "$artifact"
            '';
          };
        };

        devShells.default = pkgs.mkShell {
          name = "terminal-cell-lab";
          packages = [
            pkgs.pkg-config
            toolchain
          ];
        };

        formatter = pkgs.nixfmt;
      }
    );
}
