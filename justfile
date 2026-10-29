set shell := ["zsh", "-cu"]

# So a recipe can forward its arguments with "$@" and keep the caller's quoting.
set positional-arguments := true

# The interpreter pyo3 compiles against: the one uv manages, else the system's.
export PYO3_PYTHON := `uv python find '>=3.10' 2>/dev/null || command -v python3`

# list recipes
default:
    @just --list

# install the pinned tools and the git hooks, on a fresh clone
setup:
    cargo install --locked cargo-llvm-cov --version 0.9.1
    cargo install --locked cargo-audit --version 0.22.2
    command -v prek >/dev/null || brew install prek
    command -v gitleaks >/dev/null || brew install gitleaks
    command -v shellcheck >/dev/null || brew install shellcheck
    @just duckdb
    @just hooks

# the DuckDB CLI the read tests run their SQL through, at the version CI pins, with its iceberg extension
duckdb:
    #!/usr/bin/env zsh
    set -euo pipefail
    want=1.5.6
    if command -v duckdb >/dev/null && duckdb --version | grep -q "^v$want "; then
        duckdb -c "INSTALL iceberg" >/dev/null
        exit 0
    fi
    case "$(uname -s)" in
    Darwin) url=https://github.com/duckdb/duckdb/releases/download/v$want/duckdb_cli-osx-universal.zip
            sha=80a80c68736bd7dea53e8b02447e9759db0c8596ea156476ba396c44f54c5810 ;;
    Linux)  url=https://github.com/duckdb/duckdb/releases/download/v$want/duckdb_cli-linux-amd64.zip
            sha=6e89deac1ebbc36eed0291caf8b567b030c7b86ac35998f71854e22b3c5d5e2f ;;
    *)      echo "no pinned duckdb for $(uname -s)" >&2; exit 1 ;;
    esac
    scripts/fetch-tool.sh "$url" "$sha" duckdb "$HOME/.local/bin"
    "$HOME/.local/bin/duckdb" -c "INSTALL iceberg" >/dev/null
    echo "make sure $HOME/.local/bin comes first on PATH"

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
    cargo build --release --locked -p phasor-cli
    mkdir -p bin && cp target/release/phasor bin/phasor

# run the release binary
run *args:
    cargo run --release -q -p phasor-cli -- "$@"

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

# shellcheck over every shell script the repository tracks
shellcheck:
    git ls-files -z '*.sh' '*.bash' | xargs -0 shellcheck

# ── tests ────────────────────────────────────────────────────────────────────

# every unit and integration test
test:
    cargo test --workspace --locked

# tests with line coverage, held to the floors in coverage.json: the Rust
# tests, then the Python tests over the module built with the same
# instrumentation, so phasor-py's lines count too. Everything builds into
# target/llvm-cov-target, as cargo llvm-cov does, leaving target/debug alone.
cover:
    #!/usr/bin/env zsh
    set -euo pipefail
    mkdir -p coverage
    export CARGO_TARGET_DIR=$PWD/target/llvm-cov-target
    source <(cargo llvm-cov show-env --export-prefix 2>/dev/null)
    cargo llvm-cov clean --workspace
    cargo test --workspace --locked
    just py-bin dev
    uv sync --locked --group dev
    uv run maturin develop --uv
    uv run pytest -q --no-cov
    cargo llvm-cov report --lcov --output-path coverage/rust.lcov
    cargo run -q -p xtask -- covergate coverage/rust.lcov

# the real nemweb and WA host: a month listed per kit and the smallest file fetched (needs the network)
net-test:
    PHASOR_NET_TESTS=1 cargo test --locked -p phasor-nem -p phasor-gas -p phasor-wem --test net -- --nocapture

# ── benchmarks ───────────────────────────────────────────────────────────────

# where an ingest's time goes, in zones (zonetrace): the synthetic zip, or
# the zips given
zones *zips:
    cargo run -q --release -p phasor-bench --features zones -- {{zips}}

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

# copy phasor-schemas' tables and manifest at REV into schemas/{market}/pulled/
schemas-pull rev from="../phasorlake-schemas":
    cargo run -q -p xtask -- schemas-pull {{ rev }} --from {{ from }}

# fail unless each schemas/{market}/pulled/ is its manifest's, at the rev Cargo.lock pins
schemas-check:
    cargo run -q -p xtask -- schemas-check

# ── repository hygiene ───────────────────────────────────────────────────────

# every dependency between the workspace's crates is one layers.json declares
layers:
    cargo run -q -p xtask -- layers

# llm/index.md is loaded into every agent session through CLAUDE.md: it stays
# small, and every path and recipe it names must exist.
llm-check:
    cargo run -q -p xtask -- llm-check

# docs/errors.md lists every diagnostic code; regenerate it
errors-doc:
    cargo run -q -p xtask -- errors-doc --write

# fail if any tracked file or path names the project's old name
old-name:
    ! git grep -I -l -i -e 'uti''lake'
    ! git ls-files | grep -i -e 'uti''lake'

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

# ── python ───────────────────────────────────────────────────────────────────

# the phasor and phasor-mirror binaries the wheel ships, into python/phasorlake/bin/
py-bin profile="release":
    #!/usr/bin/env zsh
    set -euo pipefail
    cargo build --profile {{ profile }} --locked -p phasor-cli -p phasor-mirror
    dir="${CARGO_TARGET_DIR:-target}/$([ {{ profile }} = dev ] && echo debug || echo {{ profile }})"
    mkdir -p python/phasorlake/bin
    cp "$dir/phasor" "$dir/phasor-mirror" python/phasorlake/bin/

# the package into .venv, for the tests and for use: the binaries, the module, the dev tools
py-dev: (py-bin "dev")
    uv sync --locked --group dev
    uv run maturin develop --uv

# the wheel, into dist/
py-wheel: py-bin
    uv run --locked --group dev maturin build --release --out dist

# rewrite the Python in place
py-fmt:
    uv run --locked --group dev ruff format python scripts
    uv run --locked --group dev ruff check --fix python scripts

# fail on a formatting diff, without rewriting
py-fmt-check:
    uv run --locked --group dev ruff format --check python scripts

# every rule ruff has
py-lint:
    uv run --locked --group dev ruff check python scripts

# ty, a warning an error
py-type:
    uv run --locked --group dev ty check --error-on-warning

# the stubs against the built module
py-stubs: py-dev
    uv run python -m mypy.stubtest phasorlake._native --mypy-config-file pyproject.toml

# where a Python call's time goes, in zones (zonetrace): the module built
# with the feature zones, then the synthetic zip and the head fixtures, or
# the files given. `just py-dev` puts the plain module back.
py-zones *files: py-bin
    uv sync --locked --group dev
    uv run maturin develop --uv --release --features zones
    uv run python scripts/py-zones.py "$@"

# the Python tests, every line of the package covered
py-test: py-dev
    uv run pytest --cov --cov-report=term-missing
