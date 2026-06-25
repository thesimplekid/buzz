{
  description = "Buzz development runtime, services, and agent harnesses";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixpkgs-main.url = "github:NixOS/nixpkgs/master";
  };

  outputs = { self, nixpkgs, nixpkgs-main }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      nixosModules = {
        buzz-relay = import ./nix/modules/buzz-relay.nix { inherit self; };
        default = self.nixosModules.buzz-relay;
      };

      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          lib = pkgs.lib;
          pkgsMain = import nixpkgs-main {
            inherit system;
            config.allowUnfreePredicate = pkg:
              builtins.elem (lib.getName pkg) [
                "claude-code"
              ];
          };

          rustPackages = [
            "buzz-relay"
            "buzz-admin"
            "buzz-cli"
            "buzz-acp"
            "buzz-agent"
            "buzz-dev-mcp"
            "buzz-tui"
            "git-credential-nostr"
            "git-sign-nostr"
            "sprig"
          ];

          rustBins = [
            "buzz-relay"
            "buzz-reindex-kind0"
            "buzz-admin"
            "buzz"
            "buzz-acp"
            "buzz-agent"
            "buzz-dev-mcp"
            "buzz-tui"
            "git-credential-nostr"
            "git-sign-nostr"
            "sprig"
          ];

          source = lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let
                root = toString ./.;
                rel = lib.removePrefix "${root}/" (toString path);
                base = baseNameOf path;
              in
              !(base == ".git"
                || base == ".jj"
                || base == "target"
                || base == "node_modules"
                || lib.hasPrefix ".git/" rel
                || lib.hasPrefix ".jj/" rel
                || lib.hasPrefix "target/" rel
                || lib.hasInfix "/target/" rel
                || lib.hasInfix "/node_modules/" rel);
          };

          buzzRuntime = pkgs.rustPlatform.buildRustPackage {
            pname = "buzz-runtime";
            version = "0.1.0";

            src = source;
            cargoLock = {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };

            cargoBuildFlags = lib.concatMap (package: [ "-p" package ]) rustPackages;
            doCheck = false;

            nativeBuildInputs = with pkgs; [
              cmake
              pkg-config
            ];

            buildInputs = with pkgs; [
              openssl
            ] ++ lib.optionals stdenv.isDarwin [
              darwin.apple_sdk.frameworks.Security
              darwin.apple_sdk.frameworks.SystemConfiguration
            ];

            installPhase = ''
              runHook preInstall

              mkdir -p "$out/bin"
              for bin in ${lib.concatStringsSep " " rustBins}; do
                bin_path="$(find target -type f -path "*/release/$bin" -perm -0100 | head -n 1)"
                if [ -z "$bin_path" ]; then
                  echo "Could not find built binary: $bin" >&2
                  find target -type f -perm -0100 >&2
                  exit 1
                fi
                install -Dm755 "$bin_path" "$out/bin/$bin"
              done

              runHook postInstall
            '';

            meta = {
              description = "Buzz relay, CLI, TUI, and ACP runtime binaries";
              license = lib.licenses.asl20;
              mainProgram = "buzz-tui";
            };
          };

          agentAdapters = pkgs.symlinkJoin {
            name = "buzz-agent-adapters";
            paths = [
              pkgsMain.codex-acp
              pkgsMain.claude-agent-acp
            ];
            postBuild = ''
              if [ -x "$out/bin/claude-code-acp" ] && [ ! -e "$out/bin/claude-agent-acp" ]; then
                ln -sf claude-code-acp "$out/bin/claude-agent-acp"
              fi
              if [ -x "$out/bin/claude-agent-acp" ] && [ ! -e "$out/bin/claude-code-acp" ]; then
                ln -sf claude-agent-acp "$out/bin/claude-code-acp"
              fi
            '';
          };
        in
        {
          default = buzzRuntime;
          buzz-runtime = buzzRuntime;
          agent-adapters = agentAdapters;
        });

      apps = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          lib = pkgs.lib;
          packages = self.packages.${system};
          runtime = packages.buzz-runtime;
          adapters = packages.agent-adapters;

          mkScript = name: runtimeInputs: text:
            pkgs.writeShellScriptBin name ''
              set -euo pipefail
              export PATH="${lib.makeBinPath runtimeInputs}:$PATH"
              ${text}
            '';

          app = drv: name: {
            type = "app";
            program = "${drv}/bin/${name}";
          };

          serviceLib = ''
            buzz_repo_root() {
              if [ -n "''${BUZZ_REPO_ROOT:-}" ]; then
                printf '%s\n' "$BUZZ_REPO_ROOT"
              else
                printf '%s\n' "$PWD"
              fi
            }

            load_buzz_dev_env() {
              repo_root="$(buzz_repo_root)"
              if [ -f "$repo_root/.env" ]; then
                set -a
                # shellcheck disable=SC1090
                source "$repo_root/.env"
                set +a
              fi

              export DATABASE_URL="''${DATABASE_URL:-postgres://buzz:buzz_dev@localhost:5432/buzz}"
              export PGHOST="''${PGHOST:-localhost}"
              export PGPORT="''${PGPORT:-5432}"
              export PGUSER="''${PGUSER:-buzz}"
              export PGPASSWORD="''${PGPASSWORD:-buzz_dev}"
              export PGDATABASE="''${PGDATABASE:-buzz}"
              export REDIS_URL="''${REDIS_URL:-redis://localhost:6379}"
              export TYPESENSE_API_KEY="''${TYPESENSE_API_KEY:-buzz_dev_key}"
              export TYPESENSE_URL="''${TYPESENSE_URL:-http://localhost:8108}"
              export BUZZ_BIND_ADDR="''${BUZZ_BIND_ADDR:-0.0.0.0:3000}"
              export RUST_LOG="''${RUST_LOG:-buzz_relay=debug,buzz_db=debug,buzz_auth=debug,buzz_pubsub=debug,tower_http=debug}"
              export BUZZ_COMPOSE_FILE="''${BUZZ_COMPOSE_FILE:-$(buzz_repo_root)/docker-compose.yml}"
            }

            buzz_compose() {
              if docker compose version >/dev/null 2>&1; then
                docker compose -f "$BUZZ_COMPOSE_FILE" "$@"
              else
                docker-compose -f "$BUZZ_COMPOSE_FILE" "$@"
              fi
            }

            docker_health() {
              docker inspect --format '{{.State.Health.Status}}' "$1" 2>/dev/null || printf 'not_found\n'
            }

            require_docker() {
              if ! docker info >/dev/null 2>&1; then
                echo "Docker is not running or is not reachable." >&2
                exit 1
              fi
            }

            ensure_buzz_services() {
              require_docker
              buzz_compose up -d
              printf 'Waiting for Buzz services'
              for _ in $(seq 1 60); do
                pg="$(docker_health buzz-postgres)"
                redis="$(docker_health buzz-redis)"
                typesense="$(docker_health buzz-typesense)"
                minio="$(docker_health buzz-minio)"
                if [ "$pg" = healthy ] && [ "$redis" = healthy ] && [ "$typesense" = healthy ] && [ "$minio" = healthy ]; then
                  printf ' ready\n'
                  return 0
                fi
                printf '.'
                sleep 2
              done
              printf ' timed out\n' >&2
              buzz_compose ps >&2 || true
              exit 1
            }

            relay_ws_url() {
              case "''${BUZZ_RELAY_URL:-}" in
                ws://*|wss://*) printf '%s\n' "$BUZZ_RELAY_URL" ;;
                http://*) printf 'ws://%s\n' "''${BUZZ_RELAY_URL#http://}" ;;
                https://*) printf 'wss://%s\n' "''${BUZZ_RELAY_URL#https://}" ;;
                "") printf 'ws://localhost:3000\n' ;;
                *) printf '%s\n' "$BUZZ_RELAY_URL" ;;
              esac
            }
          '';

          servicesUp = mkScript "buzz-services-up" [ pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            ensure_buzz_services
            buzz_compose ps
          '';

          servicesDown = mkScript "buzz-services-down" [ pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            buzz_compose down "$@"
          '';

          servicesPs = mkScript "buzz-services-ps" [ pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            buzz_compose ps "$@"
          '';

          servicesLogs = mkScript "buzz-services-logs" [ pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            buzz_compose logs -f "$@"
          '';

          migrate = mkScript "buzz-migrate" [ runtime pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            ensure_buzz_services
            exec buzz-admin migrate "$@"
          '';

          relay = mkScript "buzz-relay-dev" [ runtime pkgs.docker pkgs.docker-compose ] ''
            ${serviceLib}
            load_buzz_dev_env
            ensure_buzz_services
            buzz-admin migrate
            exec buzz-relay "$@"
          '';

          relayOnly = mkScript "buzz-relay-only" [ runtime ] ''
            ${serviceLib}
            load_buzz_dev_env
            exec buzz-relay "$@"
          '';

          tui = mkScript "buzz-tui-dev" [ runtime adapters pkgs.nodejs ] ''
            ${serviceLib}
            load_buzz_dev_env
            export BUZZ_ORIGINAL_PATH="''${BUZZ_ORIGINAL_PATH:-$PATH}"
            export PATH="${runtime}/bin:${adapters}/bin:$PATH"
            export BUZZ_ACP_MCP_COMMAND="''${BUZZ_ACP_MCP_COMMAND:-${runtime}/bin/buzz-dev-mcp}"
            exec buzz-tui \
              --buzz-bin "${runtime}/bin/buzz" \
              --acp-bin "${runtime}/bin/buzz-acp" \
              --mcp-command "$BUZZ_ACP_MCP_COMMAND" \
              "$@"
          '';

          acpHarness = name: command: extraEnv:
            mkScript name [ runtime adapters pkgs.nodejs ] ''
              ${serviceLib}
              load_buzz_dev_env
              export BUZZ_ORIGINAL_PATH="''${BUZZ_ORIGINAL_PATH:-$PATH}"
              export PATH="${runtime}/bin:${adapters}/bin:$PATH"
              export BUZZ_RELAY_URL="$(relay_ws_url)"
              export BUZZ_ACP_AGENT_COMMAND="${command}"
              export BUZZ_ACP_AGENTS="''${BUZZ_ACP_AGENTS:-1}"
              export BUZZ_ACP_MCP_COMMAND="''${BUZZ_ACP_MCP_COMMAND:-${runtime}/bin/buzz-dev-mcp}"
              ${extraEnv}
              exec buzz-acp "$@"
            '';

          agentGoose = acpHarness "buzz-acp-goose" "goose" ''
            export GOOSE_MODE="''${GOOSE_MODE:-auto}"
          '';

          agentCodex = acpHarness "buzz-acp-codex" "codex-acp" "";
          agentClaude = acpHarness "buzz-acp-claude" "claude-agent-acp" "";
          agentBuzz = acpHarness "buzz-acp-buzz-agent" "${runtime}/bin/buzz-agent" "";

        in
        {
          default = app tui "buzz-tui-dev";
          tui = app tui "buzz-tui-dev";
          relay = app relay "buzz-relay-dev";
          relay-only = app relayOnly "buzz-relay-only";
          migrate = app migrate "buzz-migrate";
          services-up = app servicesUp "buzz-services-up";
          services-down = app servicesDown "buzz-services-down";
          services-ps = app servicesPs "buzz-services-ps";
          services-logs = app servicesLogs "buzz-services-logs";
          acp-goose = app agentGoose "buzz-acp-goose";
          acp-codex = app agentCodex "buzz-acp-codex";
          acp-claude = app agentClaude "buzz-acp-claude";
          acp-buzz-agent = app agentBuzz "buzz-acp-buzz-agent";
        });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          packages = self.packages.${system};
          basePackages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            just
            docker
            docker-compose
            nodejs
            jq
            curl
            git
            python3
            postgresql
            redis
          ];
          runtimePackages = [
            packages.buzz-runtime
            packages.agent-adapters
          ];
          commonShellHook = ''
            export BUZZ_ORIGINAL_PATH="''${BUZZ_ORIGINAL_PATH:-$PATH}"
          '';
          runtimeShellHook = ''
            ${commonShellHook}
            export BUZZ_ACP_MCP_COMMAND="''${BUZZ_ACP_MCP_COMMAND:-${packages.buzz-runtime}/bin/buzz-dev-mcp}"
          '';
        in
        {
          default = pkgs.mkShell {
            packages = basePackages ++ [
              packages.agent-adapters
            ];

            shellHook = ''
              ${commonShellHook}
              echo "Buzz dev shell: relay and TUI tooling. Use 'just relay' or 'just tui'."
            '';
          };

          runtime = pkgs.mkShell {
            packages = basePackages ++ runtimePackages;

            shellHook = ''
              ${runtimeShellHook}
              echo "Buzz runtime shell: buzz, buzz-tui, and ACP agent tools available"
            '';
          };

          agent = pkgs.mkShell {
            packages = basePackages ++ runtimePackages;

            shellHook = ''
              ${runtimeShellHook}
              echo "Buzz agent shell: buzz-acp and agent adapters available"
            '';
          };
        });
    };
}
