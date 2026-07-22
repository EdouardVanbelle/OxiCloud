{ pkgs, lib, config, ... }:

{
  languages.rust = {
    enable = true;
    channel = "stable";   # edition 2024 needs a recent stable (CLAUDE.md: Rust 1.93+)
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" "rust-src" ];
    # wasm32 std for building the WASM plugin fixtures and the vendored BLAKE3
    # module (scripts/build-plugin-hello.sh, scripts/build-wasm.sh). Only needed
    # to *rebuild* those committed artifacts, not for normal server builds/tests.
    targets = [ "wasm32-unknown-unknown" ];
  };

  # Native build deps + the full justfile toolchain.
  packages = with pkgs; [
    pkg-config
    openssl
    git

    # task runner + Rust extras
    just
    cargo-audit

    # frontend tooling (no root package.json — these are expected as global bins).
    # Pinned to Node 26 to match CI (.github/workflows/ci.yml). Newer Node ships
    # a native global Web Storage API, so a skew here vs CI silently changes
    # jsdom/localStorage behaviour in the Vitest suite.
    nodejs_26
    biome
    typescript   # provides `tsc`
    stylelint

    # helper scripts (tools/check-icons.py, tools/check-missing-translations.py)
    python3

    # API/WebDAV functional tests + the DB-readiness probe in tests/common/spawn-db.sh
    hurl
    netcat-gnu                # provides `nc`
    b3sum                     # storage-integrity check in tests/api/run.sh
  ];

  # Dev Postgres on :5432, matching DATABASE_URL in example.env
  # (postgres://postgres:postgres@localhost:5432/oxicloud).
  services.postgres = {
    enable = true;
    # Pinned to PostgreSQL 18 to match CI (postgres:18-alpine in ci.yml).
    package = pkgs.postgresql_18;
    listen_addresses = "127.0.0.1";
    port = 5432;
    initialDatabases = [ { name = "oxicloud"; } ];
    # devenv's bootstrap superuser is the OS user with trust auth, so the
    # password is not actually checked — but the `postgres` role must exist
    # for the DATABASE_URL above to connect. Extensions per CLAUDE.md.
    initialScript = ''
      CREATE ROLE postgres SUPERUSER LOGIN PASSWORD 'postgres';
      \connect oxicloud
      CREATE EXTENSION IF NOT EXISTS pg_trgm;
      CREATE EXTENSION IF NOT EXISTS ltree;
    '';
  };

  # Keep cargo's build output OUT of the repo. The devenv shell is entered via
  # `use flake "path:$PWD"` (.envrc), and a `path:` flake copies the whole working
  # tree into the Nix store WITHOUT honoring .gitignore — so a multi-GB `target/`
  # makes every direnv evaluation hang "copying … to the store". Redirecting
  # CARGO_TARGET_DIR under $HOME (per-user, so it stays portable) keeps the source
  # tree small. `builtins.getEnv` is fine here: the flake is evaluated --impure.
  env.CARGO_TARGET_DIR = "${builtins.getEnv "HOME"}/.cache/oxicloud/target";

  # Load .env (justfile uses `set dotenv-load`); contributor should `cp example.env .env`.
  dotenv.enable = true;

  enterShell = ''
    echo "OxiCloud dev env — Rust $(rustc --version | cut -d' ' -f2), Node $(node --version), Postgres on :5432"
  '';
}
