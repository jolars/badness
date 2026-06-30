{
  description = "A language server, formatter, and linter for LaTeX";

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
        pkgs = import nixpkgs {
          inherit system;
        };

        badness = pkgs.rustPlatform.buildRustPackage {
          pname = "badness";
          version = "0.4.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "distro-0.0.0" = "sha256-qPPmCn9qAzZuMDJS8AM1EVT0LomrhNgPfhcL/PAIZ0k=";
            };
          };

          nativeBuildInputs = [ pkgs.installShellFiles ];

          postInstall = ''
            installShellCompletion --cmd badness \
              --bash target/completions/badness.bash \
              --fish target/completions/badness.fish \
              --zsh target/completions/_badness

            installManPage target/man/*
          '';

          meta = with pkgs.lib; {
            description = "A language server, formatter, and linter for LaTeX";
            homepage = "https://github.com/jolars/badness";
            license = licenses.mit;
            maintainers = [ ];
          };
        };
      in
      {
        packages = {
          default = badness;
          badness = badness;
        };

        apps = {
          default = {
            type = "app";
            program = "${badness}/bin/badness";
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            go-task
            wasm-pack
            llvmPackages.bintools
          ];
        };
      }
    );
}
