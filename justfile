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
    cargo build --release --locked -p nemlake-cli
    mkdir -p bin && cp target/release/nemlake bin/nemlake

# run the release binary
run *args:
    cargo run --release -q -p nemlake-cli -- "$@"

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

# the embedded schema files: regenerate from the fixtures and fail on a diff
schema-check:
    cargo run -q -p nemlake-schema-bootstrap -- regen --check --schemas schemas --ctl testdata/bootstrap/MMSDM_2024_01/CTL --datamodel testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json --samples testdata/csv --samples testdata/bootstrap/MMSDM_2024_01/heads

# regenerate the schema files from the fixtures
schema-regen:
    cargo run -q -p nemlake-schema-bootstrap -- regen --schemas schemas --ctl testdata/bootstrap/MMSDM_2024_01/CTL --datamodel testdata/bootstrap/datamodel/mms_data_model_v5.0.0.1.json --samples testdata/csv --samples testdata/bootstrap/MMSDM_2024_01/heads

# refresh the bootstrap fixtures from nemweb: every CTL and the head of every zip (needs the network)
schema-fixtures month="2024-01":
    cargo run -q -p nemlake-schema-bootstrap -- ctls --month {{month}} --out testdata/bootstrap/MMSDM_{{replace(month, "-", "_")}}/CTL
    cargo run -q -p nemlake-schema-bootstrap -- heads --month {{month}} --out testdata/bootstrap/MMSDM_{{replace(month, "-", "_")}}/heads

# the real nemweb: one small month listing and the smallest zip (needs the network)
net-test:
    NEMLAKE_NET_TESTS=1 cargo test --locked -p nemlake-fetch --test net -- --nocapture

# ── benchmarks ───────────────────────────────────────────────────────────────

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
