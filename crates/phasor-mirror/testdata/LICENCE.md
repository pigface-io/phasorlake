# Test data

The directory listings under `listings/` are copied verbatim from nemweb
(`https://www.nemweb.com.au`), published by the Australian Energy Market
Operator (AEMO); `mmsdm_2024_01_data.html` was fetched on 2026-10-03 and the
`nemweb_*` pages on 2026-10-15; `wa_datafiles.html` is the first folders of
`https://data.wa.aemo.com.au/public/public-data/datafiles/`, fetched on
2026-10-24. They are AEMO's, used under AEMO's copyright
permissions for market data, and are not covered by this repository's
Apache-2.0 licence.

The THREDDS catalog pages `listings/thredds_p1h.xml` and
`listings/thredds_day.xml` are the catalog of NCI Australia's THREDDS
server (`https://thredds.nci.org.au/thredds/catalog/rv74/satellite-products/arc/der/himawari-ahi/solar/p1h/`,
the root and the day 2025-10-01 cut to its first three files), fetched on
2026-11-06. They describe the Bureau of Meteorology's Himawari solar
products, published through NCI under CC BY-NC 4.0; the catalog pages are
used here only to test the parser and are not covered by this repository's
Apache-2.0 licence.
