{
  pkgs,
  ...
}:

{
  packages = [
    pkgs.git
    pkgs.perf
    pkgs.cargo-flamegraph
    pkgs.cargo-llvm-cov
    pkgs.cargo-audit
    pkgs.cargo-deny
    pkgs.cargo-machete
    pkgs.go-task
    pkgs.hyperfine
    pkgs.cargo-show-asm
    pkgs.wasm-pack
    pkgs.mdbook
  ];

  languages = {
    rust = {
      enable = true;

      channel = "stable";
      version = "1.94.1";
      targets = [ "wasm32-unknown-unknown" ];
    };

    texlive = {
      enable = true;
    };
  };

  git-hooks = {
    hooks = {
      clippy = {
        enable = false;
        settings = {
          allFeatures = true;
        };
      };

      rustfmt = {
        enable = true;
      };
    };
  };
}
