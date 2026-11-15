{
  description = "the: Thepia command line for Linux and Nix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        # The same pinned compiler as rustup users get.
        craneLib = (crane.mkLib pkgs).overrideToolchain
          (p: p.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml);

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;
          pname = "thepia";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        };
        # Dependencies build once and are cached separately from the crate.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Builds and runs the full test suite (crane's default check phase).
        thepia = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          postInstall = ''
            ln -sf thepia "$out/bin/the"
            ln -sf thepia "$out/bin/th"
          '';
          meta.mainProgram = "the";
        });
      in
      {
        packages = {
          default = thepia;
          inherit thepia;
        };
        apps.default = flake-utils.lib.mkApp { drv = thepia; };

        checks = {
          inherit thepia;
          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
          });
          fmt = craneLib.cargoFmt { inherit (commonArgs) src pname version; };
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = [ pkgs.just pkgs.lychee ];
        };
      });
}
