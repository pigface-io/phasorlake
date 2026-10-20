# Licence of the schema files

The files under `nem/` describe the layout of the data files that the
Australian Energy Market Operator (AEMO) publishes on nemweb.com.au: for
each table, every distinct column list and type class observed in the
archive, the months each was seen in, the number of rows each month held,
the table's primary key and description from AEMO's MMS Data Model, and the
name and hash of the control file AEMO published for each layout. They hold
no data rows.

© AEMO. Derived from published information under AEMO's copyright
permissions (https://aemo.com.au/privacy-and-legal-notices/copyright-permissions),
which allow use of published information provided AEMO is attributed. The
derivation is reproducible: `utilake-nem-census export` writes every file
here from the archive's control files and a census of its data files, and
`provenance.json` names the commit that wrote them.

Nothing in `nem/`, `manifest.json` or `provenance.json` is licensed under
the Apache-2.0 licence that covers this repository's scripts.
