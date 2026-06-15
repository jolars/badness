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
    pkgs.maturin
  ];

  languages = {
    rust = {
      enable = true;

      toolchainFile = ./rust-toolchain.toml;
    };

    texlive = {
      enable = true;
    };

    javascript = {
      enable = true;

      pnpm = {
        enable = true;

        install = {
          enable = true;
        };
      };
    };

    typescript = {
      enable = true;
    };

    python = {
      enable = true;

      package = pkgs.python3.withPackages (ps: [
        ps.markdown
      ]);
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

      # panache = {
      #   enable = true;
      # };
    };
  };
}
