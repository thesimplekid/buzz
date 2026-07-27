{
  cmake,
  darwin,
  lib,
  openssl,
  pkg-config,
  rustPlatform,
  stdenv,
}:

rustPlatform.buildRustPackage {
  pname = "buzz-agent-runtime";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ../..;
    filter =
      path: type:
      let
        root = toString ../..;
        relative = lib.removePrefix "${root}/" (toString path);
        base = baseNameOf path;
      in
      !(
        base == ".git"
        || base == ".jj"
        || base == "target"
        || base == "node_modules"
        || lib.hasPrefix ".git/" relative
        || lib.hasPrefix ".jj/" relative
        || lib.hasPrefix "target/" relative
        || lib.hasInfix "/target/" relative
        || lib.hasInfix "/node_modules/" relative
      );
  };

  cargoLock = {
    lockFile = ../../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  cargoBuildFlags = [
    "-p"
    "buzz-acp"
    "-p"
    "buzz-agent"
    "-p"
    "buzz-dev-mcp"
  ];

  doCheck = false;

  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  buildInputs = [
    openssl
  ]
  ++ lib.optionals stdenv.isDarwin [
    darwin.apple_sdk.frameworks.Security
    darwin.apple_sdk.frameworks.SystemConfiguration
  ];

  installPhase = ''
    runHook preInstall

    mkdir -p "$out/bin"
    for binary in buzz-acp buzz-agent buzz-dev-mcp; do
      binary_path="$(find target -type f -path "*/release/$binary" -perm -0100 | head -n 1)"
      if [ -z "$binary_path" ]; then
        echo "Could not find built binary: $binary" >&2
        exit 1
      fi
      install -Dm755 "$binary_path" "$out/bin/$binary"
    done

    runHook postInstall
  '';

  meta = {
    description = "Buzz ACP harness, agent, and developer MCP runtime";
    license = lib.licenses.asl20;
    mainProgram = "buzz-acp";
  };
}
