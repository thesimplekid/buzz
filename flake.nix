{
  description = "Buzz agent runtime packages";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
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
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          buzzAgentRuntime = pkgs.callPackage ./nix/packages/buzz-agent-runtime.nix { };
        in
        {
          default = buzzAgentRuntime;
          buzz-agent-runtime = buzzAgentRuntime;

          # Compatibility alias for consumers that used the original package
          # name while migrating to the narrower agent-only runtime.
          buzz-runtime = buzzAgentRuntime;
        }
      );
    };
}
