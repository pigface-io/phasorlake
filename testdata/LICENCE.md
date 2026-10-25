# Test data

The CSV excerpts under `csv/` are the first rows of real files published by the
Australian Energy Market Operator (AEMO) on nemweb.com.au, cut to at most 20
data rows per block with their header rows intact and the footer count
corrected. The heads under `bootstrap/MMSDM_{YYYY}_{MM}/heads/` are the first
bytes of every zip in the archive of that month (`just schema-fixtures`): the
`C` header, the `I` row and at most 20 `D` rows, no footer. The heads under `heads/` are the first five rows of one real
file of every table (`just table-heads`): an MMS file's `C` row, the
table's `I` row, its `D` rows and a corrected footer, named as its DVD file;
a gas feed's file with its header row, at its path below the host. The control files
under `bootstrap/MMSDM_{YYYY}_{MM}/CTL/`, the `README.txt` beside them and
the listing under `listings/` is copied verbatim from the same archives. The JSON
under `bootstrap/datamodel/` is AEMO's MMS Data Model Report as read by
`just schema-datamodel`; the four pages under `bootstrap/datamodel/pages/`
are copied verbatim from that report.

© AEMO. Reproduced for software testing under AEMO's copyright permissions
(https://aemo.com.au/privacy-and-legal-notices/copyright-permissions), which
allow use of published information provided AEMO is attributed. Nothing here
is a complete dataset; fetch the real thing with `nemlake fetch`.
