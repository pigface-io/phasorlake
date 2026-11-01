# Distributed energy resources: what is published, what the lake holds

Assessed 2026-11-01. Rooftop solar, home batteries and the like are
"distributed energy resources" (DER). AEMO publishes little about them
beside the market data, and what it publishes is not all on nemweb. This
page says where each series is, which the lake holds, and what is decided
about the rest.

## On nemweb, in the lake

Every DER series AEMO publishes on nemweb is already a table:

| table | what | where |
|---|---|---|
| `ROOFTOP_PV_ACTUAL`, `ROOFTOP_PV_ACTUAL_AREA` | AEMO's estimate of rooftop PV generation, a half hour, by region and by load forecasting area; a measurement run and a satellite run | `Reports/ROOFTOP_PV/ACTUAL`, `ACTUAL_AREA`; MMSDM `ROOFTOP_PV_ACTUAL` |
| `ROOFTOP_PV_FORECAST`, `ROOFTOP_PV_FORECAST_AREA` | the forecast over the pre-dispatch and ST PASA horizon, mean and 10/50/90 POE | `Reports/ROOFTOP_PV/FORECAST`, `FORECAST_AREA`; MMSDM `ROOFTOP_PV_FORECAST` |
| `DEMANDOPERATIONALACTUAL`, `_AREA`, `DEMANDOPERATIONALFORECAST`, `_AREA`, `OPERATIONAL_DEMAND_ACTUAL5M` | operational demand, which is demand net of rooftop PV; carries the wholesale demand response estimate | `Reports/Operational_Demand` |

A blank rooftop reading (no POWER, or QI 0) is missing, never zero; a
region's areas sum to the region within 1 MW or 1%. Both rules and the
features that read these tables are phasorlake-datasets' (`docs/rules.md`,
`docs/datasets.md` there).

The MMS data model (v5.0) has no DER, VPP or register package. Nemweb's
`Reports/CURRENT/WDR_CAPACITY_NO_SCADA` (daily, since 2021-10, about 390
bytes a file) is wholesale demand response capacity, not DER; it is not
declared yet and goes with the other undeclared tables.

## AEMO, not on nemweb, not in the lake

### WEM estimated distributed PV

AEMO's estimate of rooftop PV in the South West Interconnected System, two
plain-CSV datafile feeds on `data.wa.aemo.com.au`:

| feed | files | step | from | header |
|---|---|---|---|---|
| `distributed-pv` | `distributed-pv-YYYY.csv` | 30 min | 2020-11 | `Trading Date,Interval Number,Trading Interval,Estimated DPV Generation (MW),Extracted At` |
| `estimated-dpv-csv` | `distributed-pv-new-YYYY.csv` | 5 min | 2023 | `Timestamp,Estimated DPV Generation(MW),Extracted At` |

A year's file is republished whole and the current year's grows. The
5-minute file is newest row first, its dates `DD/MM/YYYY`, and a year runs
by trading day (the 2024 file ends 2025-01-01 07:55). `Extracted At` is on
the first row only. The WEM kit lists and decodes these feeds today, but no
WEM table is declared: `schemas/wem/` holds only `market.toml` and
phasorlake-schemas has no WEM evidence yet. These two are the first WEM
tables to declare; see "Decided" below.

### The DER Register report

AEMO's register of DER connected in the NEM, as DNSPs report it, published
quarterly on aemo.com.au (`der-register/data-der/data-downloads`) as one
workbook a quarter, plus a dashboard; WA's register has its own dashboard.
The March 2026 file, read 2026-11-01:

- An XLSX (the 2019 guidelines said CSV), one sheet `in`, 4,093 rows, 15
  columns, every count an integer: `state`, `postcode`, `Bus_Resid`, then
  `Sum of` the number of DER sites and connections, installed DER capacity
  (kVA), solar connections, devices and capacity (kVA), battery connections,
  devices, capacity (kVA) and storage (kVAh), other DER connections and
  capacity (kVA).
- `state` is ACT, NSW, QLD, SA, TAS or VIC. `postcode` is four digits, or
  `<10 sites` on one row a state that holds every postcode with fewer than
  ten connections. `Bus_Resid` is RESIDENTIAL, BUSINESS or UNKNOWN, empty on
  the `<10 sites` rows.
- No date column: the quarter is only in the file's name. Each release
  restates earlier quarters, and from the September 2025 quarter the figures
  are cross-checked against other sources.
- aemo.com.au answers a request that is not a browser's with 403, so the
  file is downloaded by hand.

It is a stock, not a series of intervals: the installed base by postcode at
a quarter's end. Nothing in the lake decodes XLSX (the registration
workbook is kept as a document, `phasor fetch --documents`, and read by
phasorlake-datasets with DuckDB). The same route fits the register: each
quarter's workbook held as a document, read as a relation, the quarter from
the file name. Not done yet; it waits on an agreed way to fetch the file.

### Forecast inputs

The ISP and ESOO publish half-hourly traces of rooftop PV, embedded PV,
battery charging and EV charging by region, as zipped CSV on aemo.com.au.
They are scenarios, not observations, and stay out.

## Not AEMO

Nothing that is not AEMO's enters the lake: every table is derived from
AEMO's files and checked against AEMO's control files and data models. What
is worth holding beside it:

- **Clean Energy Regulator, small-scale installations by postcode**
  (`cer.gov.au/document/{name}-installations-2011-to-present-and-totals`,
  CC BY 3.0 AU). One CSV a device type: solar, battery, wind, hydro, solar
  water heater, air source heat pump; and one XLSX of them all. One row a
  postcode, one column a month since January 2011 (`Jan 2011 - Installation
  Quantity`), a 2001-2010 total and a total. Counts of systems that claimed
  certificates: an upgrade counts again, an off-grid system is in, a
  decommissioned one never leaves, a system that skipped the rebate is
  absent; no capacity. Batteries from July 2025 only. Published monthly,
  the whole file each time, and the last twelve months rise in each release
  as certificates are created. Only the current release is online. The
  longest national series of the installed base, and the one APVI's postcode
  figures derive from. Held in the archive at
  `~/.phasor/archive/cer.gov.au/document/{name}`, a version a release, and
  read by phasorlake-datasets as a reference relation, unpivoted to one row
  a postcode and month.
- **APVI**: postcode and LGA figures derived from the CER's, and live PV
  estimates behind an API key. Nothing beyond the CER plus a model.
- **AER export services data**: an annual XLSX of export customers, solar
  and battery counts by DNSP, 2020 to 2024. Regulatory, yearly; not held.
- **DNSPs**: hosting-capacity maps and quarterly PDF reports, no uniform
  feed. Ausgrid's household solar interval dataset (2011-13) has been
  withdrawn from Ausgrid's site.

## Decided

1. This page.
2. The WEM estimated distributed PV as the first WEM tables,
   `WEM_DISTRIBUTED_PV` (30 min, `TRADING_INTERVAL`) and
   `WEM_DISTRIBUTED_PV_5MIN` (`TIMESTAMP`), both `+08:00`: declarations in
   phasorlake-schemas, then `just schemas-pull`. Two fixes first in
   `crates/phasor-wem/src/sources/datafiles.rs`: the datafiles manifest of a
   yearly dataset carries `year` and `generatedAt` and no `month`, which the
   parser requires; and a file fetched once is never fetched again, so a
   growing year file is taken from the mirror, each version its own copy.
3. The CER series as a reference relation in phasorlake-datasets, fetched
   through the documents path, every release kept.
