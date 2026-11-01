{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      host = pkgs.rustPlatform.buildRustPackage {
        pname = "host";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [ "-p" "host" ];
        doCheck = false;
      };
    in {
      packages.${system}.default = host;

      apps.${system}.default = {
        type = "app";
        program = "${host}/bin/host";
        meta.description = "Run the yakui host";
      };

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          clippy
          rustfmt
          rust-analyzer
          pkg-config
          webkitgtk_4_1
          gtk3
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };
    };
}
