# PDR control files, for reference

AEMO's participant data replication (PDR) loader ships its own SQL*Loader
control files for the MMS tables. Put them here, as AEMO publishes them,
to compare them with the control files of our increments:

```sh
cargo run -q -p nemlake-census -- pdr-diff
```

It prints, per table, where a PDR control file and an increment's control
file load other columns or the same column with another loader type, and
the tables the PDR has and our schemas do not. It is a report only:
resolution never reads anything under `reference/`, and our schemas change
only through `table.toml` and `nemlake-census sync`.
