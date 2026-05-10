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
            name = "terminal-cell-session-witnesses";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test session_witnesses -- --nocapture
            '';
          };
        };

        apps.agent-terminal-witness = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-agent-terminal-witness";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test agent_terminal_witness -- --nocapture
            '';
          };
        };

        apps.daemon-witness = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-daemon-witness";
            runtimeInputs = [ toolchain ];
            text = ''
              cargo test --test daemon_witness -- --nocapture
            '';
          };
        };

        apps.ghostty-agent-demo = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-ghostty-agent-demo";
            runtimeInputs = [
              pkgs.coreutils
              toolchain
            ];
            text = ''
              cargo build \
                --bin agent-terminal-fixture \
                --bin terminal-cell-daemon \
                --bin terminal-cell-view
              target_debug="$(pwd)/target/debug"
              root="$(mktemp -d -t terminal-cell-ghostty.XXXXXX)"
              socket="$root/cell.sock"
              ready="$root/daemon.ready"
              view_ready="$root/view.ready"
              mkfifo "$ready"
              mkfifo "$view_ready"
              "$target_debug/terminal-cell-daemon" \
                --socket "$socket" \
                -- "$target_debug/agent-terminal-fixture" > "$ready" &
              daemon_pid="$!"
              ghostty_pid=""
              trap 'if [ -n "$ghostty_pid" ]; then kill "$ghostty_pid" 2>/dev/null || true; wait "$ghostty_pid" 2>/dev/null || true; fi; kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; rm -rf "$root"' EXIT
              ready_line="$(timeout 20s head -n 1 "$ready")"
              if [ -z "$ready_line" ]; then
                printf 'terminal-cell daemon did not announce readiness\n' >&2
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

              "$ghostty_bin" --class="$ghostty_class" -e "$target_debug/terminal-cell-view" --socket "$socket" --ready-file "$view_ready" &
              ghostty_pid="$!"
              view_line="$(timeout 20s head -n 1 "$view_ready")"
              if [ -z "$view_line" ]; then
                printf 'terminal-cell view did not announce attachment\n' >&2
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
            name = "terminal-cell-ghostty-agent-witness";
            runtimeInputs = [
              pkgs.coreutils
              pkgs.gnugrep
              toolchain
            ];
            text = ''
              cargo build \
                --bin agent-terminal-fixture \
                --bin terminal-cell-capture \
                --bin terminal-cell-daemon \
                --bin terminal-cell-send \
                --bin terminal-cell-view \
                --bin terminal-cell-wait
              target_debug="$(pwd)/target/debug"
              root="$(mktemp -d -t terminal-cell-ghostty-witness.XXXXXX)"
              socket="$root/cell.sock"
              daemon_ready="$root/daemon.ready"
              view_ready="$root/view.ready"
              artifact_dir="target/ghostty-agent-witness"
              artifact="$artifact_dir/transcript.txt"
              mkdir -p "$artifact_dir"
              mkfifo "$daemon_ready"
              mkfifo "$view_ready"
              "$target_debug/terminal-cell-daemon" \
                --socket "$socket" \
                -- "$target_debug/agent-terminal-fixture" > "$daemon_ready" &
              daemon_pid="$!"
              ghostty_pid=""
              trap 'if [ -n "$ghostty_pid" ]; then kill "$ghostty_pid" 2>/dev/null || true; wait "$ghostty_pid" 2>/dev/null || true; fi; kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; rm -rf "$root"' EXIT
              ready_line="$(timeout 20s head -n 1 "$daemon_ready")"
              if [ -z "$ready_line" ]; then
                printf 'terminal-cell daemon did not announce readiness\n' >&2
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

              "$ghostty_bin" --class="$ghostty_class" -e "$target_debug/terminal-cell-view" --socket "$socket" --ready-file "$view_ready" &
              ghostty_pid="$!"
              view_line="$(timeout 20s head -n 1 "$view_ready")"
              if [ -z "$view_line" ]; then
                printf 'terminal-cell view did not announce attachment\n' >&2
                exit 1
              fi
              printf '%s\n' "$view_line"

              "$target_debug/terminal-cell-wait" --socket "$socket" --text agent-ready
              "$target_debug/terminal-cell-send" --socket "$socket" --line "hello ghostty attach"
              "$target_debug/terminal-cell-wait" --socket "$socket" --text "agent-response: hello ghostty attach"
              "$target_debug/terminal-cell-capture" --socket "$socket" > "$artifact"
              grep -q "agent-response: hello ghostty attach" "$artifact"
              printf 'ghostty witness transcript=%s\n' "$artifact"
            '';
          };
        };

        apps.ghostty-agent-session = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-ghostty-agent-session";
            runtimeInputs = [
              pkgs.coreutils
              pkgs.util-linux
              toolchain
            ];
            text = ''
              cargo build \
                --bin agent-terminal-fixture \
                --bin terminal-cell-capture \
                --bin terminal-cell-daemon \
                --bin terminal-cell-send \
                --bin terminal-cell-view \
                --bin terminal-cell-wait
              target_debug="$(pwd)/target/debug"
              session_root="''${TERMINAL_CELL_SESSION_ROOT:-''${XDG_RUNTIME_DIR:-/tmp}/terminal-cell}"
              session="$session_root/session-$(date +%Y%m%d-%H%M%S)-$$"
              socket="$session/cell.sock"
              daemon_ready="$session/daemon.ready"
              view_ready="$session/view.ready"
              mkdir -p "$session"
              mkfifo "$daemon_ready"
              mkfifo "$view_ready"

              daemon_pid=""
              ghostty_pid=""
              cleanup_on_failure() {
                if [ -n "$ghostty_pid" ]; then
                  kill "$ghostty_pid" 2>/dev/null || true
                fi
                if [ -n "$daemon_pid" ]; then
                  kill "$daemon_pid" 2>/dev/null || true
                fi
                rm -rf "$session"
              }
              trap cleanup_on_failure EXIT

              setsid "$target_debug/terminal-cell-daemon" \
                --socket "$socket" \
                -- "$target_debug/agent-terminal-fixture" > "$daemon_ready" 2> "$session/daemon.stderr" &
              daemon_pid="$!"
              printf '%s\n' "$daemon_pid" > "$session/daemon.pid"
              ready_line="$(timeout 20s head -n 1 "$daemon_ready")"
              if [ -z "$ready_line" ]; then
                printf 'terminal-cell daemon did not announce readiness\n' >&2
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
              ghostty_class="''${TERMINAL_CELL_GHOSTTY_CLASS:-com.ligoldragon.terminalcellsession}"

              setsid "$ghostty_bin" --class="$ghostty_class" -e "$target_debug/terminal-cell-view" --socket "$socket" --ready-file "$view_ready" > "$session/ghostty.stdout" 2> "$session/ghostty.stderr" &
              ghostty_pid="$!"
              printf '%s\n' "$ghostty_pid" > "$session/ghostty.pid"
              view_line="$(timeout 20s head -n 1 "$view_ready")"
              if [ -z "$view_line" ]; then
                printf 'terminal-cell view did not announce attachment\n' >&2
                exit 1
              fi
              printf '%s\n' "$view_line"

              "$target_debug/terminal-cell-wait" --socket "$socket" --text agent-ready
              "$target_debug/terminal-cell-send" --socket "$socket" --line "hello durable terminal cell"
              "$target_debug/terminal-cell-wait" --socket "$socket" --text "agent-response: hello durable terminal cell"
              "$target_debug/terminal-cell-send" --socket "$socket" --line "/usage"
              "$target_debug/terminal-cell-wait" --socket "$socket" --text "usage-window: five-hour=73 weekly=41"
              "$target_debug/terminal-cell-capture" --socket "$socket" > "$session/transcript.txt"

              {
                printf 'TERMINAL_CELL_SESSION=%s\n' "$session"
                printf 'TERMINAL_CELL_SOCKET=%s\n' "$socket"
                printf 'TERMINAL_CELL_DAEMON_PID=%s\n' "$daemon_pid"
                printf 'TERMINAL_CELL_GHOSTTY_PID=%s\n' "$ghostty_pid"
                printf 'TERMINAL_CELL_GHOSTTY_CLASS=%s\n' "$ghostty_class"
              } > "$session/session.env"

              trap - EXIT
              printf 'terminal-cell session=%s\n' "$session"
              printf 'terminal-cell socket=%s\n' "$socket"
              printf 'terminal-cell transcript=%s\n' "$session/transcript.txt"
              printf 'close with: nix run .#close-ghostty-agent-sessions\n'
            '';
          };
        };

        apps.close-ghostty-agent-sessions = flake-utils.lib.mkApp {
          drv = pkgs.writeShellApplication {
            name = "terminal-cell-close-ghostty-agent-sessions";
            runtimeInputs = [
              pkgs.coreutils
            ];
            text = ''
              session_root="''${TERMINAL_CELL_SESSION_ROOT:-''${XDG_RUNTIME_DIR:-/tmp}/terminal-cell}"
              if [ ! -d "$session_root" ]; then
                printf 'no terminal-cell sessions under %s\n' "$session_root"
                exit 0
              fi

              found=0
              for session in "$session_root"/session-*; do
                [ -d "$session" ] || continue
                found=1
                for pid_file in "$session"/*.pid; do
                  [ -e "$pid_file" ] || continue
                  pid="$(cat "$pid_file")"
                  if [ -n "$pid" ]; then
                    kill "$pid" 2>/dev/null || true
                  fi
                done
                rm -rf "$session"
                printf 'closed %s\n' "$session"
              done

              if [ "$found" -eq 0 ]; then
                printf 'no terminal-cell sessions under %s\n' "$session_root"
              fi
            '';
          };
        };

        devShells.default = pkgs.mkShell {
          name = "terminal-cell";
          packages = [
            pkgs.pkg-config
            toolchain
          ];
        };

        formatter = pkgs.nixfmt;
      }
    );
}
