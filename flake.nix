{
  description = "Needledrop — a daily Heardle-style song guessing game";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust server
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            bacon

            # Svelte client
            nodejs_24
            pnpm

            just
          ];

          # rust-analyzer needs the standard library sources.
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
    };
}
