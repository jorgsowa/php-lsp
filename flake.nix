{
  description = "Nix flake for php-lsp";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
      in
      {
        packages.default = self.packages.${system}.php-lsp;
        packages.php-lsp = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          version = manifest.version;

          src = pkgs.lib.cleanSource ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            # mir-* crates are pinned to a git rev rather than crates.io;
            # replace with the hash `nix build` reports on the first run.
            outputHashes = {
              "mir-analyzer-0.65.0" = pkgs.lib.fakeHash;
            };
          };

          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

          meta = {
            description = manifest.description;
            homepage = manifest.repository;
            license = pkgs.lib.licenses.mit;
            mainProgram = "php-lsp";
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.php-lsp ];
          packages = with pkgs; [
            rust-analyzer
            rustfmt
            clippy
          ];
        };
      }
    );
}
