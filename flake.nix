{
  description = "platoon: every AI coding session you have, in one terminal app";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      packages = forAllSystems (
        system: pkgs: {
          platoon = pkgs.rustPlatform.buildRustPackage {
            pname = "platoon";
            version = cargoToml.package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            # The tests start terminals, session hosts and git in temporary folders, which the build sandbox does not
            # allow; CI runs them on Linux and macOS.
            doCheck = false;
            meta = {
              description = cargoToml.package.description;
              homepage = cargoToml.package.homepage;
              license = pkgs.lib.licenses.mit;
              mainProgram = "platoon";
              platforms = pkgs.lib.platforms.linux ++ pkgs.lib.platforms.darwin;
            };
          };
          default = self.packages.${system}.platoon;
        }
      );

      apps = forAllSystems (
        system: pkgs: {
          default = {
            type = "app";
            program = "${self.packages.${system}.platoon}/bin/platoon";
          };
        }
      );
    };
}
