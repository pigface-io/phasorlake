set shell := ["zsh", "-cu"]

# So a recipe can forward its arguments with "$@" and keep the caller's quoting.
set positional-arguments := true

# list recipes
default:
    @just --list

# install the pinned tools and the git hooks, on a fresh clone
setup:
    cargo install --locked cargo-llvm-cov --version 0.9.1
    cargo install --locked cargo-audit --version 0.22.2
    command -v prek >/dev/null || brew install prek
    command -v gitleaks >/dev/null || brew install gitleaks
    @just hooks

# install the git hooks (pre-commit and pre-push)
hooks:
    prek install --overwrite

# run every hook of both stages on every file, as CI does
check:
    prek run --all-files
    prek run --all-files --stage pre-push

# ── build ────────────────────────────────────────────────────────────────────

# the release binary, into bin/
build:
    cargo build --release --locked -p utilake-cli
    mkdir -p bin && cp target/release/utilake bin/utilake

# run the release binary
run *args:
    cargo run --release -q -p utilake-cli -- "$@"

# ── style and lint ───────────────────────────────────────────────────────────

# rewrite files in place
fmt:
    cargo fmt --all

# fail on a formatting diff, without rewriting (the pre-commit hook)
fmt-check:
    cargo fmt --all -- --check

# clippy with warnings as errors, and the docs with broken links as errors
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --quiet

# ── tests ────────────────────────────────────────────────────────────────────

# every unit and integration test
test:
    cargo test --workspace --locked

# tests with line coverage, held to the floors in coverage.json
cover:
    @mkdir -p coverage
    cargo llvm-cov --workspace --locked --lcov --output-path coverage/rust.lcov
    cargo run -q -p xtask -- covergate coverage/rust.lcov

# the real nemweb and WA host: a month listed per kit and the smallest file fetched (needs the network)
net-test:
    UTILAKE_NET_TESTS=1 cargo test --locked -p utilake-nem -p utilake-gas -p utilake-wem --test net -- --nocapture

# the read kernel against DuckDB running the same read as SQL (needs duckdb on PATH)
oracle-test:
    UTILAKE_ORACLE_TESTS=1 cargo test --locked -p utilake-query --test oracle -- --nocapture

# ── benchmarks ───────────────────────────────────────────────────────────────

# where an ingest's time goes, in zones (zonetrace): the synthetic zip, or
# the zips given
zones *zips:
    cargo run -q --release -p utilake-bench --features zones -- {{zips}}

# the fast benchmark set (parse, registry), held to bench/budgets.json
bench-fast:
    cargo run -q --release -p xtask -- bench --set fast

# every benchmark including end-to-end wall time, peak heap and RSS, held to bench/budgets.json
bench:
    cargo run -q --release -p xtask -- bench --set full

# tighten budgets to the latest measurements (never loosens)
bench-tighten:
    cargo run -q --release -p xtask -- bench --set full --tighten

# fail if a budget loosened or a coverage floor dropped against the base branch
budget-check base="origin/main":
    cargo run -q -p xtask -- ratchet --base {{ base }}

# ── schemas ──────────────────────────────────────────────────────────────────

# copy utilake-schemas' tables and manifest at REV into schemas/{market}/pulled/
schemas-pull rev from="../utilake-schemas":
    cargo run -q -p xtask -- schemas-pull {{ rev }} --from {{ from }}

# fail unless each schemas/{market}/pulled/ is its manifest's, at the rev Cargo.lock pins
schemas-check:
    cargo run -q -p xtask -- schemas-check

# ── repository hygiene ───────────────────────────────────────────────────────

# llm/index.md is loaded into every agent session through CLAUDE.md: it stays
# small, and every path and recipe it names must exist.
llm-check:
    cargo run -q -p xtask -- llm-check

# docs/errors.md lists every diagnostic code; regenerate it
errors-doc:
    cargo run -q -p xtask -- errors-doc --write

# fail if docs/errors.md is out of date
errors-check:
    cargo run -q -p xtask -- errors-doc --check

# known vulnerabilities in the Rust crates (needs the network)
audit:
    cargo audit

# gitleaks over the staged changes
secrets-staged:
    gitleaks git --pre-commit --staged --no-banner --redact

# gitleaks over the commits being pushed
secrets-push:
    scripts/gitleaks-push.sh

# gitleaks over the whole history
secrets:
    gitleaks git --no-banner --redact

# testdata/heads: the first rows of one real file per table, read from the mirror at HOME
table-heads home:
    rm -rf testdata/heads
    python3 -I scripts/table-heads.py schemas {{ home }}/archive/www.nemweb.com.au testdata/heads
