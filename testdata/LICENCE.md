# Test data

The CSV excerpts under `csv/` are the first rows of real files published by the
Australian Energy Market Operator (AEMO) on nemweb.com.au, cut to at most 20
data rows per block with their header rows intact and the footer count
corrected. The control files under `bootstrap/MMSDM_2024_01/CTL/` and the
listing under `listings/` are copied verbatim from the same archive. The JSON
under `bootstrap/datamodel/` is scraped from AEMO's MMS Data Model Report.

© AEMO. Reproduced for software testing under AEMO's copyright permissions
(https://aemo.com.au/privacy-and-legal-notices/copyright-permissions), which
allow use of published information provided AEMO is attributed. Nothing here
is a complete dataset; fetch the real thing with `nemlake fetch`.
