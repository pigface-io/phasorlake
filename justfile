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

# cut the bootstrap fixtures of a month from the local archive: every CTL, the head of every data file and the README (download the month first: `nemlake fetch nem --from MONTH`)
schema-fixtures month="2024-01":
    cargo run --release -q -p nemlake-census -- fixtures --month {{month}} --out testdata/bootstrap/MMSDM_{{replace(month, "-", "_")}}

# record every block of every archived file of each month (shape and rows) into schemas/nem/census/ and schemas/nem/shapes/; reads the local archive in full, offline
census from to=from *args:
    cargo run --release -q -p nemlake-census -- census --from {{from}} --to {{to}} "${@:3}"

# bring schemas/nem/ up to the control files of each archived month: a table a month lays out anew gains an increment, a missing table is started; nothing there is changed (take the census of the months first)
schema-sync from to=from:
    cargo run --release -q -p nemlake-census -- sync --from {{from}} --to {{to}}

# start every table's increments again from the control files of the archived months, in month order (the first pass over the whole archive); columns, ids and types stay
schema-rebuild from="2009-07" to="2026-08":
    cargo run --release -q -p nemlake-census -- sync --rebuild --from {{from}} --to {{to}}

# fail if syncing the archived months would change schemas/nem/ (a month not downloaded here is skipped)
schema-sync-check from="2024-01" to="2026-08":
    cargo run --release -q -p nemlake-census -- sync --check --from {{from}} --to {{to}}

# a new month, end to end: download it once, record every block of every file, sync the schemas to its control files, gate the census against them
census-forward month:
    cargo run --release -q -p nemlake-cli -- fetch nem --from {{month}}
    just census {{month}}
    just schema-sync {{month}}
    just census-check

# every recorded census block against our schema: its table exists, it conforms to the increment in force that month, each control-file type fits our column's (passes when no census is recorded)
census-check:
    cargo run -q -p xtask -- census-check

# read AEMO's MMS Data Model Report into testdata/bootstrap/datamodel/ (needs the network; a few hundred paced requests)
schema-datamodel interval_ms="1200":
    cargo run -q -p nemlake-census -- datamodel --interval-ms {{interval_ms}} --out testdata/bootstrap/datamodel

# the real nemweb: one small month listing and the smallest zip (needs the network)
net-test:
    NEMLAKE_NET_TESTS=1 cargo test --locked -p nemlake-acquire --test net -- --nocapture

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

# fail if a table file changed in a way the lake does not survive
schema-compat base="origin/main":
    cargo run -q -p xtask -- schema-compat --base {{ base }}

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
