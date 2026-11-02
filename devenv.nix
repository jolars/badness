{
  config,
  lib,
  pkgs,
  ...
}:

{
  packages = [
    pkgs.git
    pkgs.bashInteractive
    pkgs.perf
    pkgs.cargo-flamegraph
    pkgs.cargo-llvm-cov
    pkgs.cargo-audit
    pkgs.cargo-deny
    pkgs.cargo-machete
    pkgs.go-task
    pkgs.hyperfine
    pkgs.tex-fmt
    pkgs.texlab
    pkgs.cargo-show-asm
    pkgs.wasm-pack
    pkgs.maturin
  ];

  # The npm workerd executable needs the Nix dynamic linker on Linux.
  env = lib.optionalAttrs pkgs.stdenv.isLinux {
    MINIFLARE_WORKERD_PATH = "${pkgs.writeShellScript "badness-workerd" ''
      cd "${config.devenv.root}/docs"
      binary=$(node -e 'const {createRequire}=require("node:module"); const wrangler=createRequire(require.resolve("wrangler/package.json")); const miniflare=createRequire(wrangler.resolve("miniflare/package.json")); process.stdout.write(miniflare("workerd").default);')
      loader=$(cat "$NIX_CC/nix-support/dynamic-linker")
      exec "$loader" "$binary" "$@"
    ''}";
  };

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
    };
  };
}
