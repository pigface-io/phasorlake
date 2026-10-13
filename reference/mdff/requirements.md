# MDFF NEM12 and NEM13: requirements

Every normative requirement of AEMO's *Meter Data File Format
Specification NEM12 & NEM13*, version 2.7, effective 1 December 2025,
paraphrased. Field-level wording is quoted only where the exact text
matters. Page numbers are those printed in the spec ("Page n of 47").

## How to read this file

- **ID.** `MDFF-<GROUP>-<nnn>`. IDs are stable: a requirement is never
  renumbered, and a removed one keeps its ID with a note. The groups are:
  `FILE` (file, packaging, characters, dates), `BLOCK` (record order), one
  group per record type (`100`, `200`, `300`, `400`, `500`, `900`, `250`,
  `550`), `READ` (index and register reads), `NUM` (number formats,
  Appendix B), `UOM` (units, Appendix B), `QUAL` (quality flags,
  Appendix C), `METHOD` (method flags, Appendix D), `REASON` (reason codes,
  Appendices E and F) and `TRANS` (transaction codes, Appendix A).
- **Level.** MUST, MUST NOT, SHOULD or MAY, as the spec intends it. The
  spec writes "must", "must not", "should", "may", "is to be", "is
  required" and "is not required"; these are mapped to the nearest level.
  A field marked `M` in a record table is a MUST; `R` (provide if
  available) is a MUST that a validator cannot check, because it cannot
  know what was available; `N` is a MAY.
- **Source.** Section of the spec and printed page.
- **Violation.** An input that breaks the rule, for a validator test.
  "Not checkable" means the rule concerns facts outside the file (MSATS,
  the meter, the agreement between participants).
- **Derived.** A rule marked *derived* is not written in the spec as such
  but follows directly from rules that are, as stated in the item. Nothing
  else here goes beyond the text.
- **Spec issue.** Where the text is ambiguous, contradicts itself, or its
  own example breaks its rules, the item says so and gives the page to
  check. None of these is resolved by guessing.

## FILE: the file as a whole

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-FILE-001 | MUST | The file is CSV. | §2(a)(i), p.5 | Tab-separated or fixed-width records. |
| MDFF-FILE-002 | MUST | A file contains exactly one header (100) record. **Spec issue:** §2(a) says one 100 and one 900 per file, but the 100 table says "1 per file (100-900 record set)", §3.2.2 speaks of "the 100 Header Record(s)" and §2(c) of the "100-900 event block", which leaves room for several 100-900 blocks when MDFF is carried as CSV blocks inside another format (§1.1(b), p.5). For a standalone file, apply one. | §2(a)(ii), p.5; §4.2, p.9; §3.2.2, p.6 | Two `100` records in one standalone file. |
| MDFF-FILE-003 | MUST | A file contains exactly one end (900) record. | §2(a)(iii), p.5 | No `900` record; two `900` records. |
| MDFF-FILE-004 | MUST | A file holds NEM12 or NEM13 data, never both. | §2(a)(iv), p.5 | A `250` record in a file whose 100 record says `NEM12`; a `300` record under `NEM13`. |
| MDFF-FILE-005 | MUST | Every NMI suffix of a NMI for a given IntervalDate is in the same 100-900 block. **Spec issue:** §4.5 (p.13) points to "section 2 (c) for further rules regarding the use of" the 400 record; §2(c) says nothing about 400 records, so the cross-reference looks misplaced. | §2(c), p.6 | NMI `QTST000001` suffix `E1` for 20250101 in one file and suffix `B1` for the same date in another. Not checkable within a single standalone file. |
| MDFF-FILE-006 | MUST | The format of interval metering data is `NEM12`; of accumulated metering data, `NEM13`. The 100 record's VersionHeader says which. | §3.1, p.6 | Interval (300) data under `100,NEM13,…`. |
| MDFF-FILE-007 | MUST | Under the alternative delivery method, a standalone file is named `VersionHeader#UniqueID#From#To.csv`, e.g. `nem12#0123456789012345#mda1#retail1.csv`. Delivery through the B2B e-Hub follows the B2B Procedure: Technical Delivery Specification instead (§3.2.1), which is outside this spec. | §3.2.2(a)(iii), p.6 | `nem12_0123_mda1_retail1.csv`; a name with three `#`-separated parts. |
| MDFF-FILE-008 | MUST | The file name is not case sensitive: a reader compares it case-insensitively. | §3.2.2(a)(i), p.6 | A reader that rejects `NEM12#ABC#MDA1#RET1.CSV` (the violation is in the reader). |
| MDFF-FILE-009 | MUST NOT | `#` is the delimiter in the file name and is not used within any of the file header (100) fields. | §3.2.2(a)(ii), p.6 | `100,NEM12,202501011200,MD#1,RET1`. |
| MDFF-FILE-010 | MUST | The name's VersionHeader is 5 alphanumeric characters, `NEM12` or `NEM13`, and matches the VersionHeader of the 100 record. | §3.2.2(a)(iii), p.6 | File `nem13#…#…#….csv` whose 100 record says `NEM12`. |
| MDFF-FILE-011 | MUST | The name's Unique ID is at most 36 alphanumeric characters and is unique. | §3.2.2(a)(iii), p.6 | A 37-character ID; `abc-123` (hyphen is not alphanumeric). Uniqueness is not checkable from one file. |
| MDFF-FILE-012 | MUST | The name's From is the Participant ID of the MDP that generates the file; To is the Participant ID of the intended Registered Participant, MDP or ENM. These are the definitions of the 100 record's FromParticipant and ToParticipant; the spec does not say in words that name and record must be equal, so a validator may warn on a mismatch but should not cite this ID as broken. | §3.2.2(a)(iii), p.6 | Not checkable beyond the comparison with the 100 record. |
| MDFF-FILE-013 | MAY | A file may be delivered compressed with a `.zip` extension, provided "the 'zlib' standard is used to manage the compression". **Spec issue:** zlib is a library and a stream format, not a zip compression method; the practical reading is a zip archive using DEFLATE. The spec does not say how many entries the zip holds or how the entry is named (p.6-7). | §3.2.2(b)(i), p.6 | A `.zip` that uses a compression method other than DEFLATE or stored (by the practical reading). |
| MDFF-FILE-014 | MUST NOT | A compressed file is not password protected. | §3.2.2(b)(ii), p.6 | A zip with an encrypted entry. |
| MDFF-FILE-015 | MUST | A compressed standalone file is named `VersionHeader#UniqueID#From#To.zip`, e.g. `nem12#0123456789012345#mda1#retail1.zip`. | §3.2.2(b)(iii), p.6-7 | `nem12#0123#mda1#retail1.csv.zip`. |
| MDFF-FILE-016 | MUST | Field values are not case sensitive, except where the allowed values are an enumerated list in the record definition tables of sections 4 and 5, which are case sensitive. A reader compares free text and identifiers case-insensitively. **Spec issue:** the record tables give some lists inline (`Allowed value: NEM12`, DirectionIndicator `I`/`E`) and others only by reference (TransCode → Appendix A, QualityMethod → Appendices C and D, ReasonCode → Appendix E, UOM → Appendix B). Whether referenced lists count as "enumerated in the record definition tables" is not said; Appendix B states that UOM is *not* case sensitive (MDFF-UOM-002), and §4.6 asks for "capital letter O" in TransCode, which suggests TransCode is. Check p.7, p.14, p.22. | §3.3(a), p.7 | `100,nem12,…` (inline enumeration in lower case); DirectionIndicator `e`. |
| MDFF-FILE-017 | MUST | Every record line ends in carriage return and line feed (CRLF), the last (900) line included. | §3.3(b), p.7 | A file with LF-only line ends; a `900` with no line end. |
| MDFF-FILE-018 | MUST NOT | Fields have no leading or trailing spaces. **Spec issue:** several of the spec's own examples break this: the 250 example on p.16 (`20040509, 20040202125010`), H.3 on p.35 (`, 200312200000001`), H.8 on p.40 (`N1, MET34567`, `kWh, 20031108`) and the NEM13 examples on p.43-44 (`20040407, 20040108100333` and similar). | §3.3.1(a), p.7 | `300,20250101, 1.5,…`. |
| MDFF-FILE-019 | MUST NOT | IntervalValue (NEM12 300 record) and Quantity (NEM13 250 record) are never null. | §3.3.1(b), p.7 | `300,20250101,1.5,,1.5,…` (an empty interval value). |
| MDFF-FILE-020 | MUST | A comma separates every pair of fields, even when a field is null: every record carries its full field count. | §3.3.1(c), p.7 | `400,1,48,A` (4 fields; the record has 6). |
| MDFF-FILE-021 | MUST NOT | No data field contains a comma. The spec defines no quoting or escaping, so a field cannot carry one. | §3.3.1(d), p.7 | ReasonDescription `meter, damaged` (shifts every later field). |
| MDFF-FILE-022 | MUST | Every component of a date or date-time field is two digits (century, year, month, day, hour, minute, second). | §3.3.2(a), p.7 | `2025111` for 1 November 2025. |
| MDFF-FILE-023 | MUST | `Date(8)` is `CCYYMMDD`, always 8 characters, no separators, e.g. `20030501` is 1 May 2003. *Derived:* it names a real calendar date. | §3.3.2(b), p.7 | `2025-05-01`; `250501`; `20250230`. |
| MDFF-FILE-024 | MUST | `DateTime(12)` is `CCYYMMDDhhmm`, always 12 characters, e.g. `200301011534` is 15:34 on 1 January 2003. | §3.3.2(c), p.7 | `20030101153` (11 characters); `2003-01-01 15:34`. |
| MDFF-FILE-025 | MUST | `DateTime(14)` is `CCYYMMDDhhmmss`, always 14 characters, e.g. `20030101153445` is 15:34:45 on 1 January 2003. **Spec issue:** H.3 (p.35) and I.5 (p.44) print 15-digit values (`200312200000001`, `200311090000000`); they are typos. | §3.3.2(d), p.7 | `200312200000001` (15 characters). |
| MDFF-FILE-026 | MUST | The end of a day is written as 00:00 of the following day. *Derived:* an hour of `24` is not used. | §3.3.2(e), p.7 | `202501012400`. |
| MDFF-FILE-027 | MUST | Field requirement codes: `M` mandatory (always provided, never null); `R` required (provided if the information is available); `N` not required (may be provided; the recipient may ignore it). Where a table gives two or three codes (`M/N`, `N/M`, `R/N`, `M/R/N`), the field's definition says which applies when; this file states each case in the field's item. | §3.3.6, p.8-9 | An empty `M` field, e.g. `200,,E1,…` (no NMI). |

## BLOCK: record order

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-BLOCK-001 | MUST | NEM12 records come in the order 100, 200, 300, 400, 500, 900. Read from the blocking diagram (p.9, a picture with no text equivalent) and Appendix G (p.32), the order is `100 ( 200 ( 300 400* 500* )+ )+ 900`: after a 300 come its 400 records, then its 500 records; after those, another 300 (same 200) or another 200, or the 900. The diagram marks 100, 200, 300 and 900 "Mandatory" and 400 and 500 "Required" (only in certain circumstances, MDFF-400-002 and MDFF-500-002). | §4.1, p.9; App G, p.32 | `400` before any `300`; `500` straight after a `200`; `300` before any `200`; any record after `900`. |
| MDFF-BLOCK-002 | MUST | Every 200 record is followed by at least one 300 record. | §4.1 diagram, p.9 | `200,…` followed directly by another `200` or by `900`. |
| MDFF-BLOCK-003 | MUST | When any data in the 200 record changes (e.g. UOM, IntervalLength or NMISuffix), a new 200 record precedes the next 300 record. | §4.1, p.9 | A day of 5-minute data under a 200 record with IntervalLength 30 (shows as MDFF-300-003). |
| MDFF-BLOCK-004 | MAY | One 200 record may carry several 300-500 blocks (several days). | §4.3, p.10 | None (a permission). |
| MDFF-BLOCK-005 | MUST | NEM13 records come in the order 100, 250, 550, 900. From the diagram (p.15) and Appendix G (p.32): `100 ( 250 550* )+ 900`; 550 is "Required" only in certain circumstances (MDFF-550-002). | §5.1, p.15; App G, p.32 | `550` before any `250`; a `200` or `300` in a NEM13 file. |

## 100: header record (NEM12 and NEM13)

Five fields: `RecordIndicator,VersionHeader,DateTime,FromParticipant,ToParticipant`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-100-001 | MUST | RecordIndicator, `Numeric(3)`, M: `100`. It is the first record of the file. | §4.2, p.9; §5.2, p.16 | A file whose first record is `200`. |
| MDFF-100-002 | MUST | VersionHeader, `VarChar(5)`, M: `NEM12` in a NEM12 file, `NEM13` in a NEM13 file ("Allowed value"). Case sensitive by MDFF-FILE-016. | §4.2, p.9; §5.2, p.16 | `NEM14`; `NEM12` above `250` records. |
| MDFF-100-003 | MUST | DateTime, `DateTime(12)`, M: the file's creation date and time. | §4.2, p.9; §5.2, p.16 | `20250101` (8 characters). |
| MDFF-100-004 | MUST | FromParticipant, `VarChar(10)`, M: Participant ID of the MDP that generates the file. | §4.2, p.10; §5.2, p.16 | Empty; `MDPPARTICIPANT1` (15 characters). |
| MDFF-100-005 | MUST | ToParticipant, `VarChar(10)`, M: Participant ID of the intended Registered Participant, MDP or ENM. | §4.2, p.10; §5.2, p.16 | Empty; longer than 10 characters. |
| MDFF-100-006 | MUST | The record has exactly 5 fields. | §4.2, p.9; MDFF-FILE-020 | `100,NEM12,202501011200,MDP1`. |
| MDFF-100-007 | MUST | A 100 record has a matching 900 record. | §4.2, p.9; §5.2, p.16 | See MDFF-FILE-003. |

## 200: NMI data details record (NEM12)

Ten fields: `RecordIndicator,NMI,NMIConfiguration,RegisterID,NMISuffix,MDMDataStreamIdentifier,MeterSerialNumber,UOM,IntervalLength,NextScheduledReadDate`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-200-001 | MUST | RecordIndicator, `Numeric(3)`, M: `200`. | §4.3, p.10 | — |
| MDFF-200-002 | MUST | NMI, `Char(10)`, M: the NMI of the connection point, without check digit or suffix. | §4.3, p.10 | `QTST0000011` (11 characters, check digit included). |
| MDFF-200-003 | MUST | NMIConfiguration, `VarChar(240)`, M: the string of all NMI suffixes that apply to the NMI, representing the Site's actual configuration (e.g. `E1Q1`, `E1B1Q1E2`). *Derived:* it contains the record's NMISuffix as one of its 2-character suffixes (every spec example does). | §4.3, p.10 | Empty; `E1` with NMISuffix `B1`. |
| MDFF-200-004 | MUST | Where the NMI configuration changes, NMIConfiguration lists every channel active on any part of the day. | §4.3, p.10 | Not checkable from one record. |
| MDFF-200-005 | MUST / MAY | RegisterID, `VarChar(10)`, M/N: the interval meter register identifier, as the RegisterID of the CATS_Register_Identifier table (e.g. `1`, `2`, `E1`, `B1`). Mandatory for type 4, 4A and 5 metering data when the sender is the Current MDP; not required for types 1-3 and 7, or when sending to another MDP (e.g. meter churn). | §4.3, p.10 | Empty RegisterID for type 5 data sent by the Current MDP (the type is not in the file, so only checkable with standing data). |
| MDFF-200-006 | SHOULD | RegisterID matches the value in MSATS. | §4.3, p.10 | Not checkable. |
| MDFF-200-007 | MUST | NMISuffix, `Char(2)`, M: as defined in the NMI Procedure (e.g. `E1`, `B1`, `Q1`, `K1`). | §4.3, p.10 | `E` (1 character). |
| MDFF-200-008 | MUST / MAY | MDMDataStreamIdentifier, `Char(2)`, M/N: the suffix field of the CATS_NMI_DataStream table (e.g. `N1`, `N2`). Provided if the metering data has been or would be sent to MSATS by the sender; not required when sending to another MDP. When given, it matches MSATS. | §4.3, p.10 | `N` (1 character). |
| MDFF-200-009 | MUST / MAY | MeterSerialNumber, `VarChar(12)`, M/N: the Meter Serial ID of the meter installed. After a replacement, the new meter's ID applies on and from the IntervalDate of the replacement. Not required for type 7 installations, logical meters, Historical Data, or where several meters are summated to one RegisterID. | §4.3, p.11 | A 13-character serial. |
| MDFF-200-010 | MUST | UOM, `VarChar(5)`, M: one of the Appendix B units (MDFF-UOM-001). | §4.3, p.11 | `kwhr`; empty. |
| MDFF-200-011 | MUST | IntervalLength, `Numeric(2)`, M: the interval in minutes, `5`, `15` or `30`. | §4.3, p.11 | `10`; `60`; `05` (the spec does not say whether leading zeros are allowed; see "Not specified"). |
| MDFF-200-012 | MUST / MAY | NextScheduledReadDate (NSRD), `Date(8)`, M/N: not required for remotely read meters, nor where the meter will not be read again (meter removed, NMI abolished, MDP no longer the MDP). The value is accurate when the file is made and may change; MSATS prevails on a discrepancy. | §4.3, p.11 | `2025-04-01`. |
| MDFF-200-013 | MUST | The record has exactly 10 fields. | §4.3, p.10; MDFF-FILE-020 | `200,QTST000001,E1,1,E1,N1,MTR1,kWh,30` (9 fields). |

## 300: interval data record (NEM12)

Fields: `RecordIndicator,IntervalDate,IntervalValue1,…,IntervalValueN,QualityMethod,ReasonCode,ReasonDescription,UpdateDateTime,MSATSLoadDateTime`, where N = 1440 / IntervalLength.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-300-001 | MUST | RecordIndicator, `Numeric(3)`, M: `300`. | §4.4, p.12 | — |
| MDFF-300-002 | MUST | IntervalDate, `Date(8)`, M. | §4.4, p.12 | `2025010` |
| MDFF-300-003 | MUST | The number of IntervalValues equals 1440 divided by the IntervalLength of the governing 200 record: 288 for 5 minutes, 96 for 15, 48 for 30. **Spec issue:** examples H.7 (p.40, both days, both files) and H.8 File 2 (p.40-41) print 52 values under IntervalLength 30. | §4.4, p.12 | 47 values under IntervalLength 30. |
| MDFF-300-004 | MUST | IntervalValue, `Numeric(sx.y)`, M: the energy or other measured value for the interval, inclusive of any multiplier or scaling factor. Never null (MDFF-FILE-019). Format by UOM in Appendix B (MDFF-NUM-001). | §4.4, p.12 | An empty value; `abc`. |
| MDFF-300-005 | MUST NOT | An IntervalValue is not negative. | §4.4, p.12 | `-1.5`. |
| MDFF-300-006 | MAY / MUST NOT | An IntervalValue may have decimal places; it is never in exponential notation. | §4.4, p.12 | `1.5E3`; `2e-1`. |
| MDFF-300-007 | MUST | Intervals run in time order. Interval 1 is the first interval after midnight of the IntervalDate; interval *n* is the period ending *n* × IntervalLength minutes after 00:00 of the IntervalDate. The last interval (288, 96 or 48) ends at 00:00 of IntervalDate + 1. For example interval 1 of 5-minute data ends at 00:05, of 15-minute data at 00:15, of 30-minute data at 00:30. | §3.3.3, p.7-8 | Not checkable from the file; a writer that puts the 00:00-00:05 value last breaks it. |
| MDFF-300-008 | MUST | QualityMethod, `VarChar(3)`, M: the quality and method for all IntervalValues of the record, in the form `QMM` (one-character quality flag, two-character method flag; MDFF-QUAL-003). Allowed values from Appendices C and D. With quality flag `A` or `V`, no method flag is required. | §4.4, p.12 | `S` (substituted with no method flag); `X14`. |
| MDFF-300-009 | MUST | Where the same QualityMethod and ReasonCode apply to every IntervalValue of the day, they are given in the 300 record itself (QualityMethod, ReasonCode, ReasonDescription), with no 400 records. | §4.4, p.11 | `300,…,V,,,…` followed by a single `400,1,48,A,,`. |
| MDFF-300-010 | MUST | Where QualityMethod or ReasonCode differ across the IntervalValues, the 300 record's QualityMethod is `V` and 400 records are provided. | §4.4, p.11-12 | QualityMethod `A` on a day whose intervals were partly substituted (only checkable by the recipient against other data). |
| MDFF-300-011 | MUST | Where the quality flag is `A` for the whole day and reason code 61, 79 or 89 applies to part of it, the 300 record's QualityMethod is `V` and 400 records are provided. | §4.4, p.11; §3.3.5(c), p.8 | Not checkable from the file alone. |
| MDFF-300-012 | MUST | `V` in the 300 record means the QualityMethod, ReasonCode or ReasonDescription vary across the day; they are given per interval in the 400 records that immediately follow this 300 record. | §4.4, p.12 | `300,…,V,…` followed by `500` or `300` with no `400`. |
| MDFF-300-013 | MUST / MAY / MUST NOT | ReasonCode, `Numeric(3)`, M/N: the reason for all IntervalValues of the record. Mandatory with quality flag `S` or `F` (MDFF-QUAL-006). Not required with `A` or `E`, but may be given for information. Not populated with `V` (§3.3.5(g)). Values from Appendix E (MDFF-REASON-001). | §4.4, p.12; §3.3.5, p.8 | `F14,,` (no reason code with F); `V,32,`. |
| MDFF-300-014 | MUST / MAY | ReasonDescription, `VarChar(240)`, N/M: description of the ReasonCode; mandatory where the ReasonCode is `0`. | §4.4, p.12 | `S53,0,,` (code 0 with no description). |
| MDFF-300-015 | MUST / MAY | UpdateDateTime, `DateTime(14)`, M/N: the MDP's version date-time, the latest time any IntervalValue or QualityMethod of the IntervalDate was created or changed; it applies to the data of this record. **Spec issue:** the field is M/N but the definition does not say when it is not required; every spec example fills it. Check p.12-13. | §4.4, p.12-13 | `2025010212` (10 characters). |
| MDFF-300-016 | MUST | MSATSLoadDateTime, `DateTime(14)`, R: the date-time MSATS recorded on loading the data, as given in MSATS's acknowledgement to the MDP. | §4.4, p.13 | `202501021200` (12 characters). |
| MDFF-300-017 | MUST | 300 records are in date order: each IntervalDate is later than the previous 300 record's, whether the days are consecutive or not. **Spec issue:** the scope is not stated; example H.6 (p.38-39) repeats IntervalDate 20040810 and 20040811 under successive 200 records, so the rule holds within one 200 record. | §4.4, p.11 | Under one 200 record: `300,20250102,…` then `300,20250101,…`; or the same date twice. |
| MDFF-300-018 | MUST | The record has 2 + N + 5 fields: 295 for 5-minute, 103 for 15-minute and 55 for 30-minute data. | §4.4, p.12; MDFF-FILE-020 | A 30-minute record with 54 fields. |

## 400: interval event record (NEM12)

Six fields: `RecordIndicator,StartInterval,EndInterval,QualityMethod,ReasonCode,ReasonDescription`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-400-001 | MUST | RecordIndicator, `Numeric(3)`, M: `400`. | §4.5, p.13 | — |
| MDFF-400-002 | MUST | 400 records are present when the preceding 300 record's QualityMethod is `V`. **Spec issue:** the spec does not say in words that a 400 record may not follow a non-`V` 300 record, but example H.7 (p.40) is the only place it happens (a 300 with `A` followed by `400,1,20,A,,` and `400,21,48,E52,,`), and that example has 52 values, so it reads as an error; MDFF-300-009 says a uniform day carries no 400 records. | §4.5, p.13 | `300,…,V,,,…` followed by `500` or `900`. |
| MDFF-400-003 | MUST | StartInterval, `Numeric(4)`, M: the first interval the record applies to. EndInterval, `Numeric(4)`, M: the last. StartInterval ≤ EndInterval. Both lie in 1 … 1440 / IntervalLength. | §4.5, p.13 | `400,30,20,A,,`; `400,1,49,A,,` under IntervalLength 30. |
| MDFF-400-004 | MUST | The StartInterval/EndInterval pairs of a day's 400 records come in ascending order. | §4.5, p.13 | `400,27,48,…` before `400,1,26,…`. |
| MDFF-400-005 | MUST | The 400 records of a day cover the whole day without gaps or overlaps: the first starts at 1, each next StartInterval is the previous EndInterval + 1, the last ends at 1440 / IntervalLength. | §4.5, p.13 | `400,1,20,…` then `400,22,48,…` (gap at 21); `400,1,20,…` then `400,20,48,…` (overlap at 20); last ends at 47. |
| MDFF-400-006 | MUST | QualityMethod, `VarChar(3)`, M: in the form `QMM`, applying to every interval of the inclusive range. Values from Appendices C and D. Quality flag `A` needs no method flag. Quality flag `V` is not allowed in a 400 record. | §4.5, p.13-14 | `400,1,48,V,,`; `400,1,48,S,9,`. |
| MDFF-400-007 | MUST / MAY | ReasonCode, `Numeric(3)`, M/N: applies to every interval of the range. Not required with `E` (may be given for information). Mandatory with `S` and `F` (MDFF-QUAL-006) and with `A` on intervals where the meter recorded a power outage (79), time reset (89) or tamper (61) (MDFF-QUAL-008); other codes may be given with `A`. Values from Appendix E. **Spec issue:** the 400 table names only `E` as not requiring a reason code; the `A` case comes from §3.3.5(c)-(d) and Appendix C, and the spec's own 400 examples carry `A` with no code (p.13). | §4.5, p.14; §3.3.5, p.8 | `400,1,48,S53,,`. |
| MDFF-400-008 | MUST / MAY | ReasonDescription, `VarChar(240)`, N/M: applies to the range; mandatory where the ReasonCode is `0`. | §4.5, p.14 | `400,1,48,F52,0,`. |
| MDFF-400-009 | MAY | A 300 record may have several 400 records where more than one ReasonCode applies to the day. | §3.3.5(e), p.8 | None (a permission). |
| MDFF-400-010 | MUST | The record has exactly 6 fields. | §4.5, p.13; MDFF-FILE-020 | `400,1,48,A`. |
| MDFF-400-011 | — | **Spec issue, no rule:** the second 400 example on p.13 prints `400,61,62.A,89,` (a full stop for a comma, also in the PDF), and covers intervals 1-288 (a 5-minute day) right after a 30-minute example. | §4.5, p.13 | — |

## 500: B2B details record (NEM12)

Five fields: `RecordIndicator,TransCode,RetServiceOrder,ReadDateTime,IndexRead`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-500-001 | MUST | RecordIndicator, `Numeric(3)`, M: `500`. | §4.6, p.14 | — |
| MDFF-500-002 | MUST | A 500 record is present where a manual Meter Reading has been performed or attempted. | §4.6, p.14 | Not checkable from the file. |
| MDFF-500-003 | MUST | Only valid 500 records for the current Meter Reading period are provided: one tied to a Substitute stops being valid once Actual Metering Data replaces the Substitute. | §4.6, p.14 | Not checkable from one file. |
| MDFF-500-004 | MUST | The record is repeated where several TransCodes or RetServiceOrders apply to the day. | §4.6, p.14 | Not checkable from the file. |
| MDFF-500-005 | MUST | TransCode, `Char(1)`, M: why the recipient receives the data; a value of Appendix A (MDFF-TRANS-001). `O` (capital letter O) when providing Historical Data and the information is unavailable. | §4.6, p.14 | `500,X,,…`; `500,0,,…` (digit zero). |
| MDFF-500-006 | MUST | RetServiceOrder, `Varchar(15)`, R: the service order number tied to the Meter Reading. | §4.6, p.14 | A 16-character value. |
| MDFF-500-007 | MUST | ReadDateTime, `DateTime(14)`, M/R/N: the actual date-time of the Meter Reading, or for a Substitution (quality flag `S` or `F`), when the reading should have occurred. Required when providing Historical Data; not required for Estimates. **Spec issue:** the definition gives the R and N cases; it does not say which case makes the field M (p.14-15). | §4.6, p.14-15 | `20250102` (8 characters). |
| MDFF-500-008 | SHOULD / MUST | ReadDateTime's time is the actual time of the attempted Meter Reading; if that is not available, the time is `000001` (00:00:01). | §4.6, p.14-15 | Not checkable (a time of `000000` is not proof). |
| MDFF-500-009 | MUST / MAY | IndexRead, `Varchar(15)`, R/N: the total accumulated energy of the Datastream read from the register at collection. For type 4A and type 5 installations the MDP provides it when collected (MDFF-READ-002). | §4.6, p.15 | A 16-character value. |
| MDFF-500-010 | MUST | The record has exactly 5 fields. | §4.6, p.14; MDFF-FILE-020 | `500,N,,20250102143000`. |
| MDFF-500-011 | MUST | *Derived* (MDFF-BLOCK-001 and "apply to the day"): a 500 record follows the 300 record, and its 400 records, of the day it belongs to. | §4.6, p.14; examples H.3 p.34, H.6 p.39 | `500` placed before the `300` of its day. |

## 900: end record (NEM12 and NEM13)

One field: `RecordIndicator`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-900-001 | MUST | RecordIndicator, `Numeric(3)`, M: `900`, the only field. It ends the record set begun by the previous 100 record. | §4.7, p.15; §5.5, p.20 | `900,` (two fields). |
| MDFF-900-002 | MUST | *Derived* (MDFF-FILE-002, MDFF-FILE-003, MDFF-BLOCK-001/005): in a standalone file the 900 record is the last record. | §2(a), p.5 | Any record after `900`. |

## 250: accumulation meter data record (NEM13)

Twenty-three fields: `RecordIndicator,NMI,NMIConfiguration,RegisterID,NMISuffix,MDMDataStreamIdentifier,MeterSerialNumber,DirectionIndicator,PreviousRegisterRead,PreviousRegisterReadDateTime,PreviousQualityMethod,PreviousReasonCode,PreviousReasonDescription,CurrentRegisterRead,CurrentRegisterReadDateTime,CurrentQualityMethod,CurrentReasonCode,CurrentReasonDescription,Quantity,UOM,NextScheduledReadDate,UpdateDateTime,MSATSLoadDateTime`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-250-001 | MUST | RecordIndicator, `Numeric(3)`, M: `250`. | §5.3, p.16 | — |
| MDFF-250-002 | MUST | NMI, `Char(10)`, M: without check digit or suffix. | §5.3, p.16 | An 11-character NMI. |
| MDFF-250-003 | MUST | NMIConfiguration, `VarChar(240)`, M: all NMI suffixes that apply to the NMI, representing the Site's actual configuration (e.g. `1141`, `7111`). | §5.3, p.16 | Empty. |
| MDFF-250-004 | MUST | RegisterID, `VarChar(10)`, M: the accumulation meter register identifier, as in CATS_Register_Identifier (e.g. `1`, `2`); it should match MSATS. Unlike the 200 record, it is always mandatory. | §5.3, p.16 | Empty RegisterID. |
| MDFF-250-005 | MUST | NMISuffix, `Char(2)`, M: as in the NMI Procedure (e.g. `11`, `41`). | §5.3, p.16 | `1`. |
| MDFF-250-006 | MUST / MAY | MDMDataStreamIdentifier, `Char(2)`, M/N: the suffix of CATS_NMI_DataStream (e.g. `11`, `41`); matches MSATS; provided if the data has been or would be sent to MDM. (The 200 record says "MSATS" where this says "MDM", p.10 vs p.17.) | §5.3, p.16-17 | `1`. |
| MDFF-250-007 | MUST | MeterSerialNumber, `VarChar(12)`, M: as per Standing Data for MSATS. Unlike the 200 record, always mandatory. | §5.3, p.17 | Empty. |
| MDFF-250-008 | MUST | DirectionIndicator, `Char(1)`, M: `I` or `E`, said from the grid's side. `I` = "Import to grid": energy normally flows from the connection point to the grid. `E` = "Export from grid": energy normally flows from the grid to the connection point (an ordinary consumption register is `E`, as in every NEM13 example). | §5.3, p.17 | `X`; `i` (case sensitive, MDFF-FILE-016). |
| MDFF-250-009 | MUST | PreviousRegisterRead, `Varchar(15)`, M: the earlier of the two Meter Readings, in the register read format of MDFF-READ-007. | §5.3, p.17 | Empty. |
| MDFF-250-010 | MUST NOT | PreviousRegisterRead is not an Estimate: PreviousQualityMethod's quality flag is not `E`. | §5.3, p.17 | `…,000100,20250101000000,E64,,,…` in the previous-read fields. |
| MDFF-250-011 | MUST | PreviousRegisterReadDateTime, `DateTime(14)`, M: the actual date-time of the reading, or for a Substitution (`S`, `F`) when it should have occurred. The time component should be the actual time of the attempted reading, and is `000001` (00:00:01) if that is not available. | §5.3, p.17 | Empty; 12 characters. |
| MDFF-250-012 | MUST | PreviousQualityMethod, `VarChar(3)`, M: `QMM` for the previous read, values from Appendices C and D; `A` needs no method flag. `V` is not allowed (MDFF-QUAL-009). | §5.3, p.17 | `V`; `S` with no method flag. |
| MDFF-250-013 | MUST / MAY | PreviousReasonCode, `Numeric(3)`, M/N: values from Appendix E; not required with `A` or `E` but may be given; mandatory with `S` or `F` (MDFF-QUAL-006). | §5.3, p.17 | `F64,,` in the previous-read fields. |
| MDFF-250-014 | MUST / MAY | PreviousReasonDescription, `VarChar(240)`, N/M: mandatory where PreviousReasonCode is `0`. | §5.3, p.17 | `0,` with an empty description. |
| MDFF-250-015 | MUST | CurrentRegisterRead, `Varchar(15)`, M: the later of the two readings, in the format of MDFF-READ-007. It need not be in the past: an Estimate can lie in the future. | §5.3, p.17-18 | Empty. |
| MDFF-250-016 | MUST / SHOULD | CurrentRegisterReadDateTime, `DateTime(14)`, M: as MDFF-250-011 for the current read. For Estimates, the date should be on or after the NextScheduledReadDate, with time `000000`. | §5.3, p.18 | An `E` current read dated before the NSRD (SHOULD); 12 characters (MUST). |
| MDFF-250-017 | MUST | CurrentQualityMethod, `VarChar(3)`, M: as MDFF-250-012 for the current read. | §5.3, p.18 | `V`. |
| MDFF-250-018 | MUST / MAY | CurrentReasonCode, `Numeric(3)`, M/N: as MDFF-250-013 for the current read. | §5.3, p.18 | `S64,,`. |
| MDFF-250-019 | MUST / MAY | CurrentReasonDescription, `VarChar(240)`, N/M: mandatory where CurrentReasonCode is `0`. | §5.3, p.18 | `0,` with an empty description. |
| MDFF-250-020 | MUST | Quantity, `Numeric(sx.y)`, M: the computed quantity after any multiplier and any meter rollover. For energy (e.g. Wh, varh) it is CurrentRegisterRead less PreviousRegisterRead, corrected for the register multiplier; for demand it is CurrentRegisterRead corrected for the multiplier. Never negative, never null (MDFF-FILE-019). Format by UOM in Appendix B. | §5.3, p.18; §3.3.1(b), p.7 | `-5`; empty. The arithmetic is not checkable without the multiplier. |
| MDFF-250-021 | MUST | UOM, `VarChar(5)`, M: the unit of the Quantity, one of Appendix B (MDFF-UOM-001). | §5.3, p.18-19 | `kwhr`. |
| MDFF-250-022 | MUST / MAY | NextScheduledReadDate, `Date(8)`, M/N: the NSRD; not required where the meter will not be read again (meter removed, NMI abolished, MDP no longer the MDP). Accurate when the file is made; MSATS prevails. | §5.3, p.19 | `2025-04-10`. |
| MDFF-250-023 | MUST | UpdateDateTime, `DateTime(14)`, M: the MDP's version date-time for the latest change to CurrentRegisterRead or CurrentQualityMethod; applies to this record. (In the 300 record it is M/N; here it is M.) | §5.3, p.19 | Empty. |
| MDFF-250-024 | MUST | MSATSLoadDateTime, `DateTime(14)`, R: as in the 300 record. | §5.3, p.19 | 12 characters. |
| MDFF-250-025 | MUST | The record has exactly 23 fields. | §5.3, p.16; MDFF-FILE-020 | 22 fields. |
| MDFF-250-026 | MUST | *Derived* from "earlier" and "later" (MDFF-250-009, MDFF-250-015): PreviousRegisterReadDateTime is not after CurrentRegisterReadDateTime. They may be equal (example I.5, p.44, an opening read). | §5.3, p.17-18 | Previous `20250301000001`, current `20250201000001`. |

## 550: B2B details record (NEM13)

Five fields: `RecordIndicator,PreviousTransCode,PreviousRetServiceOrder,CurrentTransCode,CurrentRetServiceOrder`.

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-550-001 | MUST | RecordIndicator, `Numeric(3)`, M: `550`. | §5.4, p.19 | — |
| MDFF-550-002 | MUST / MAY | The record is not required when both PreviousTransCode and CurrentTransCode would be `N` and no service order relates to either read; otherwise it is provided (the diagram marks it "Required", p.15). | §5.4, p.19 | Not checkable from the file: a 250 with no 550 cannot show which TransCodes applied. |
| MDFF-550-003 | MUST | The record is repeated where several TransCodes or RetServiceOrders apply to the previous or current read. | §5.4, p.19 | Not checkable. |
| MDFF-550-004 | MUST | PreviousTransCode, `Char(1)`, M: why the previous read was collected; a value of Appendix A; `O` for Historical Data where the information is unavailable. | §5.4, p.19 | `550,X,,N,`. |
| MDFF-550-005 | MUST | PreviousRetServiceOrder, `Varchar(15)`, R: the retailer's ServiceOrderRequest number tied to the previous read, where the data is directly tied to one. It is provided only to the requesting retailer. | §5.4, p.19 | A 16-character value. Who requested it is not checkable. |
| MDFF-550-006 | MUST | CurrentTransCode, `Char(1)`, M: as MDFF-550-004 for the current read. | §5.4, p.20 | `550,N,,,` (empty). |
| MDFF-550-007 | MUST | CurrentRetServiceOrder, `Varchar(15)`, R: as MDFF-550-005 for the current read; only to the requesting retailer. | §5.4, p.20 | A 16-character value. |
| MDFF-550-008 | MUST | The record has exactly 5 fields. | §5.4, p.19; MDFF-FILE-020 | `550,N,,A`. |
| MDFF-550-009 | MUST | *Derived* (MDFF-BLOCK-005): a 550 record follows the 250 record it describes. | §5.1, p.15 | `550` before any `250`. |

## READ: index reads and register reads

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-READ-001 | — | Definition: the index read is the total accumulated metering data of a Datastream, read from the meter's register at collection (rules 002-006 are for type 4A and type 5 installations). | §3.3.4(a), p.8 | — |
| MDFF-READ-002 | MUST | IndexRead is provided for active energy (Wh) Datastreams when collected. | §3.3.4(b), p.8; §4.6, p.15 | A manually read type 5 day with a 500 record whose IndexRead is empty (type not in the file). |
| MDFF-READ-003 | MUST NOT | Registered Participants do not raise a Validation query with the MDP about an IndexRead value. A process rule. | §3.3.4(c), p.8 | Not checkable. |
| MDFF-READ-004 | MUST | IndexRead excludes meter multipliers, and is in the format displayed on the meter, with any leading or trailing zeros where available. *Derived:* a reader keeps it as text; parsing it as a number loses the zeros. | §3.3.4(d), p.8 | Not checkable from the file. |
| MDFF-READ-005 | MUST | An IndexRead for any time other than the current Meter Reading is agreed between the Registered Participant and the MDP. | §3.3.4(e), p.8 | Not checkable. |
| MDFF-READ-006 | MUST | Where several index reads were collected on the Meter Reading date, only the latest is provided. | §3.3.4(f), p.8 | Two 500 records for one day with the same TransCode and different IndexReads (suggestive only). |
| MDFF-READ-007 | MUST | PreviousRegisterRead and CurrentRegisterRead (250 record), `Varchar(15)`: include leading and trailing zeros as on the physical dial, and exclude meter multipliers (e.g. `1234567.123`, `0012456.123`). *Derived:* kept as text, as MDFF-READ-004. | §5.3, p.17-18 | A 16-character read. |

## NUM: number formats (Appendix B)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-NUM-001 | MUST | IntervalValue (300) and Quantity (250) follow the format of their UOM: `M…` (mega) units `Numeric 15.7`; `k…` (kilo) units `Numeric 15.4`; `pf` `Numeric 15.3`; `Wh`, `VArh`, `VAh`, `VAr`, `VA`, `V`, `A`, `W` `Numeric 15.1`. The column is headed "CHARACTER LENGTH" and the text calls it "the format and maximum number of characters". Reading: at most 15 characters, at most 7 / 4 / 3 / 1 digits after the decimal point. **Spec issue:** the spec does not say whether the 15 counts the decimal point, whether the decimals are a maximum or an exact count, nor what `s`, `x` and `y` in `Numeric(sx.y)` (p.12, p.18) stand for. The spec's own examples use fewer decimals than the maximum (e.g. `1.1112` kWh). Check p.22. | App B, p.22 | `1.12345` in kWh (5 decimals); `12.34` in Wh (2 decimals); `1234567890123456` (16 characters). |
| MDFF-NUM-002 | — | Other fields have no specific number format beyond their `Numeric(n)` length. | App B, p.22 | — |

## UOM: units of measure (Appendix B)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-UOM-001 | MUST | UOM (200 and 250 records) is one of the 23 values below. | App B, p.22 | `kwhr`; `GWh`; `mW` is read as `MW` (MDFF-UOM-002), not rejected. |
| MDFF-UOM-002 | MUST | UOM values are not case sensitive. Case-insensitive matching is unambiguous for this list (no two values differ only by case), so `mwh` means MWh: there are no milli units. | App B, p.22 | A reader that rejects `KWH`. |

| UOM | Description (Appendix B) | Number format |
|---|---|---|
| `MWh` | megawatt hour | 15.7 |
| `kWh` | kilowatt hour | 15.4 |
| `Wh` | watt hour | 15.1 |
| `MVArh` | megavolt ampere reactive hour (megavar hour) | 15.7 |
| `kVArh` | kilovolt ampere reactive hour | 15.4 |
| `VArh` | volt ampere reactive hour | 15.1 |
| `MVAr` | megavolt ampere reactive | 15.7 |
| `kVAr` | kilovolt ampere reactive | 15.4 |
| `VAr` | volt ampere reactive | 15.1 |
| `MW` | megawatt | 15.7 |
| `kW` | kilowatt | 15.4 |
| `W` | watt | 15.1 |
| `MVAh` | megavolt ampere hour | 15.7 |
| `kVAh` | kilovolt ampere hour | 15.4 |
| `VAh` | volt ampere hour | 15.1 |
| `MVA` | megavolt ampere | 15.7 |
| `kVA` | kilovolt ampere | 15.4 |
| `VA` | volt ampere | 15.1 |
| `kV` | kilovolt | 15.4 |
| `V` | volt | 15.1 |
| `kA` | kiloampere | 15.4 |
| `A` | ampere | 15.1 |
| `pf` | Power Factor | 15.3 |

## QUAL: quality flags (Appendix C)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-QUAL-001 | MUST | The quality flag (first character of a QualityMethod) is one of `A`, `E`, `F`, `S`, `V`, with the meanings in the table below. | App C, p.23 | `X14`; `N`. |
| MDFF-QUAL-002 | MUST NOT | There is no `N` quality flag. It was removed in version 2.2 (ICF_025, "Removal of 'N' Metering Data Quality Flag", p.47) and is absent from Appendix C. Missing data is not sent as a null either (MDFF-FILE-019). | App C, p.23; history, p.47 | `300,…,N,,,…`; `400,1,10,N,,`. |
| MDFF-QUAL-003 | MUST | QualityMethod is `QMM`: a one-character quality flag followed by a two-character method flag; with `A` or `V` the method flag is left out (so the field is `A` or `V`). | §4.4, p.12; §4.5, p.13; §5.3, p.17 | `S5` (one-character method); `E521`. |
| MDFF-QUAL-004 | MAY | With `A`, no method flag is required. **Spec issue:** "not required" does not forbid one; whether `A14` is valid is not stated, and every example writes plain `A`. | App C, p.23 | — |
| MDFF-QUAL-005 | MUST | With `E`, `F` or `S`, a method flag is mandatory. | App C, p.23 | `E`; `S`; `F` (no method flag). |
| MDFF-QUAL-006 | MUST | With `F` or `S`, a reason code is mandatory, for every interval and every consumption value. | App C, p.23; §3.3.5(b), p.8 | `S53,,`. |
| MDFF-QUAL-007 | MAY | With `E`, a reason code may be given for information. **Spec issue:** Appendix C says "No reason code applies if this quality flag is used", while the 300 (p.12), 400 (p.14) and 250 (p.17-18) tables say a reason code is not required with `E` "but can be provided for information", and reason code 77 "Estimation Forecast" is described as applicable to Estimations (p.29). A validator should accept a reason code with `E`. | App C, p.23; §4.4, p.12; §4.5, p.14 | — |
| MDFF-QUAL-008 | MUST | With `A`, a reason code is mandatory for every interval where the meter recorded a power outage (79), time reset (89) or tamper (61). Other reason codes may be given with `A` (§3.3.5(d)). | App C, p.23; §3.3.5(c)-(d), p.8 | Not checkable from the file. |
| MDFF-QUAL-009 | MUST NOT | `V` is used only as the QualityMethod of a NEM12 300 record: never in a 400 record, never in a NEM13 file. With `V`, no method flag and no reason code apply. `V` is not a quality flag held against any data item. | App C, p.23; §4.5, p.14; §3.3.5(g), p.8 | `400,1,48,V,,`; `250,…,V,,,…`; `300,…,V,32,…`. |
| MDFF-QUAL-010 | MUST | One interval has exactly one QualityMethod and one ReasonCode. | §3.3.5(f), p.8 | Overlapping 400 ranges (MDFF-400-005). |

| Flag | Meaning (Appendix C) | Method flag | Reason code | Where |
|---|---|---|---|---|
| `A` | Actual Metering Data (as defined in Metrology Procedure Part B) | not required | not required; mandatory on intervals with 79, 89 or 61; others optional | 300, 400, 250 |
| `E` | Forward estimated data (as defined in Metrology Procedure Part B) | mandatory | "no reason code applies" (Appendix C); optional "for information" (record tables) | 300, 400, 250 (current read only) |
| `F` | Final substituted data (as defined in Metrology Procedure Part B) | mandatory | mandatory | 300, 400, 250 |
| `S` | Substituted data (as defined in Metrology Procedure Part B) | mandatory | mandatory | 300, 400, 250 |
| `V` | Variable data; not a formal quality flag held against individual data items | none | none | 300 only |

The Appendix C cell for `V` repeats its own meaning text in the "Relationship
with other fields" column; this is in the PDF, not an extraction fault (p.23).

## METHOD: method flags (Appendix D)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-METHOD-001 | MUST | The method flag is the two-character number of a substitution or estimation type in table "2.6 Summary Table of Substitution and Estimation types" of Metrology Procedure: Part B (method flag 17 is substitution type 17). This spec lists no values: the allowed set lives in the Metrology Procedure, which is not part of this reference. Method flags that appear in the spec's examples are `14`, `52`, `53`, `54`, `56` and `64`; that is not an allowed list. | App D, p.24 | `S5` (one character); `SAB` (not a number). Whether a two-digit number is a current type is checkable only against the Metrology Procedure. |
| MDFF-METHOD-002 | MUST | A method flag is present with `E`, `F` and `S`, absent with `V`, and not required with `A` (MDFF-QUAL-004/005/009). | App C, p.23 | `F,32,`. |

## REASON: reason codes (Appendices E and F)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-REASON-001 | MUST | A ReasonCode is one of Appendix E (table below). | App E, p.25-30 | `56` (in neither appendix); `110`. |
| MDFF-REASON-002 | MAY | The obsolete codes of Appendix F are used only when providing Historical Data. | §3.3.5(h), p.8; App F, p.31 | `90` in current data (the file does not say whether data is historical, so only checkable with context). |
| MDFF-REASON-003 | MUST | The MDP applies the ReasonCode that most accurately reflects the reason, or follows the hierarchy agreed with the FRMP. A process rule. | §3.3.5(a), p.8 | Not checkable. |
| MDFF-REASON-004 | MUST | Where the ReasonCode is `0` (free text), its ReasonDescription is given (300, 400, and both reads of 250). | §4.4, p.12; §4.5, p.14; §5.3, p.17-18 | `S53,0,,`. |
| MDFF-REASON-005 | MUST NOT | No ReasonCode with QualityMethod `V`. | §3.3.5(g), p.8; §4.4, p.12 | `300,…,V,79,…`. |
| MDFF-REASON-006 | MAY | Other ReasonCodes may be given with quality flag `A`. | §3.3.5(d), p.8 | — |
| MDFF-REASON-007 | — | **Spec issue, no rule:** codes 80 and 81 describe intervals "not being a full 15 or 30 minutes" / "exceeding a full 15 or 30 minutes", not mentioning 5-minute data (p.29). | App E, p.29 | — |

### Appendix E: reason codes in use

| Code | Name | Description (paraphrased) |
|---|---|---|
| 0 | Free text description | When no other code can reasonably be used, e.g. system issues preventing collection or delivery of market data. Needs a ReasonDescription. |
| 1 | Meter/equipment changed | The metering installation has changed. |
| 2 | Extreme weather conditions | Extreme weather (floods, storms, bushfires) prevented data collection. |
| 3 | Quarantined premises | Premises under quarantine or lockdown prevent access (e.g. pandemic). |
| 5 | Blank screen | Electronic meter display is blank; may be powered off or faulty, cannot tell which. |
| 6 | De-energised premises | Blank electronic meter where the reader can tell the Site is de-energised, or an interval metered Site de-energised with Datastreams left active, for which the MDP substitutes. |
| 7 | Unable to locate meter | Site found but the metering installation was not. |
| 8 | Vacant premises | Reader believes the Site is vacant and could not access the meter. |
| 9 | Under investigation | An issue with the metering installation is being investigated. |
| 10 | Lock damaged unable to open | A damaged lock prevents access. |
| 11 | In wrong route | No reading because the installation is in the wrong route. |
| 12 | Locked premises | Site locked, no access to the installation. |
| 13 | Locked gate | A locked gate prevents access. |
| 14 | Locked meter box | A locked meter box prevents access. |
| 15 | Overgrown vegetation | Overgrown vegetation prevents access. |
| 17 | Unsafe equipment/location | Equipment or location identified as unsafe (other than a high meter). |
| 18 | Read less than previous | Current reading below the previous one, no sign of tampering, no reverse energy observed. |
| 20 | Damaged equipment/panel | Equipment or panel damaged but not identified as unsafe. |
| 21 | Main switch off | Blank electronic meter where the reader can tell the main switch is off, or an interval metered Site with the main switch off and Datastreams left active, for which the MDP substitutes. |
| 22 | Meter/equipment seals missing | One or more seals missing, no tampering identified. |
| 23 | Reader error | MDP found the reader's Meter Reading incorrect. |
| 24 | Substituted/replaced data (data correction) | Interval Meter Reading: MDP replaced erroneous data for specific intervals. |
| 25 | Unable to locate premises | Site could not be found. |
| 26 | Negative consumption (generation) | Accumulation meter whose previous reading exceeds the current one; the Site usually has generation. |
| 27 | RoLR | Transfer of the End User because of a RoLR Event. |
| 28 | CT/VT fault | MDP corrected data for a known instrument transformer (CT/VT) fault. |
| 29 | Relay faulty/damaged | Reader found the relay device faulty. |
| 31 | Not all meters read | MDP has not received readings for every meter of the Site (typically a non-scheduled reading). |
| 32 | Re-energised without readings | No readings because of exceptional circumstances when the Site was re-energised outside standard practice. |
| 33 | De-energised without readings | No readings at de-energisation, including disconnection for non-payment. |
| 34 | Meter not in handheld | Unexpected (new or additional) meter found on Site. |
| 35 | Timeswitch faulty/reset required | Reader found the time switching device faulty and needing a reset. |
| 36 | Meter high/ladder required | Meter too high to read without a ladder. |
| 37 | Meter under churn | MDP substituted because data was not received from the relevant MDP. |
| 38 | Unmarried lock | Two or more locks, one a power industry lock, not interlocked to allow access. |
| 39 | Reverse energy observed | Reverse energy where the Site is not expected to have it. |
| 40 | Unrestrained livestock | Roaming farm animals are a hazard, or access was not taken because they might escape. |
| 41 | Faulty Meter display/dials | Display or dials faulty; Site not de-energised and display not blank. |
| 42 | Channel added/removed | Data for an added channel, or substitutes for a removed channel whose Datastream is still active in MSATS. |
| 43 | Power outage | Interval meter: intervals substituted because power was not available at the installation. |
| 44 | Meter testing | Substitutes reflecting consumption while the meter was under test. |
| 45 | Readings failed to validate | Readings loaded but failed Validation and were substituted. |
| 47 | Refused access | The End User refused access. |
| 48 | Dog on premises | A dog on Site; reader could not tell whether it is dangerous. |
| 51 | Installation demolished | The metering installation no longer exists. |
| 52 | Access – blocked | Items block safe access to the meter or Site. |
| 53 | Pests in meter box | Pests in the meter box risk accuracy, safety or the reader. |
| 54 | Meter box damaged/faulty | Meter box damaged or faulty; mechanical protection or weather proofing compromised. |
| 55 | Dials obscured | Dials obscured (painted, misted, faded, mouldy panel …); no evidence of tampering. |
| 60 | Illegal connection | The Site has been illegally connected. |
| 61 | Equipment tampered | Installation tampered with; recorded consumption may be affected. |
| 62 | NSRD window expired | The NSRD window expired without Actual Meter Readings. |
| 64 | Key required | Reader usually has the key but could not get it at the time. |
| 65 | Wrong key provided | The key provided no longer opens the lock. |
| 67 | Transfer | Reading provided to enable a customer transfer. |
| 68 | Zero consumption | Known zero consumption; Site energised in MSATS but no energy flows to the meter. |
| 69 | Reading exceeds Substitute | Re-substituted data smoothed to align with the next Actual Meter Reading. |
| 71 | Probe read error | The meter probe could not extract the data. |
| 72 | Re-calculated based on Actual Metering Data | Actual readings received; prior Substitutes amended. |
| 73 | Low consumption | Reading failed Validation as too low against Historical Data; kept as actual or substituted. |
| 74 | High consumption | Reading failed Validation as too high against Historical Data; kept as actual or substituted. |
| 75 | Customer read | Reading provided by the End User (where jurisdictions allow it). |
| 76 | Communications fault | Remote communication with the meter failed. |
| 77 | Estimation Forecast | Optional code for Estimations. |
| 78 | Null Data | Interval meter: no data received; Substitutes cover the period. |
| 79 | Power Outage Alarm | Interval meter detected a power outage. |
| 80 | Short Interval Alarm | Meter clock was slow and was corrected, so interval data is shorter than a full interval. |
| 81 | Long Interval Alarm | Meter clock was fast and was corrected, so interval data exceeds a full interval. |
| 87 | Reset occurred | Meter reset for reprogramming, configuration change, firmware upgrade and the like. |
| 89 | Time reset occurred | A time reset occurred within the installation. |
| 100 | Incorrect Meter Multiplier | Correcting data after the wrong meter multiplier was applied. |
| 101 | Temporarily Connection Point unmetered | The connection point was temporarily unmetered (e.g. mains by-pass). |
| 102 | Customer By-Pass | The customer by-passed the meter. |
| 103 | Network By-Pass | The network by-passed the meter, believing it faulty, to supply the customer. |
| 104 | Transposed Channel | Meter data streams transposed (e.g. ToU with controlled load). |
| 105 | Transposed Channel - UoM Correction | Data channels transposed across units (e.g. kWh with kvarh). |
| 106 | Transposed Channel – Reverse Polarity | Meter wired in reverse, or a reverse polarity alarm, swapping export and import registers. |
| 107 | Transposed Meter | Correcting data after crossed meters. |
| 108 | Network by-pass extreme weather | The network by-passed the meter because extreme weather affected it. |
| 109 | Defined load method | Retailer or LNSP provides profile data from an off-market meter or other measured data that best represents the load. |

Codes 4, 16, 19, 30, 46, 49, 50, 58, 70, 82-86, 88 and 90-99 are not in
Appendix E; they are the obsolete codes below. Codes 56, 57, 59, 63, 66,
and 110 and above appear in neither appendix.

### Appendix F: obsolete reason codes (Historical Data only)

| Code | Name |
|---|---|
| 4 | Dangerous dog |
| 16 | Noxious weeds at premises are preventing access to metering installation |
| 19 | Consumer wanted |
| 30 | Meter stop switch on |
| 46 | Extreme weather/hot |
| 49 | Wet paint |
| 50 | Wrong tariff |
| 58 | Meter ok – supply failure |
| 70 | Probe reports tampering |
| 82 | CRC error |
| 83 | RAM checksum error |
| 84 | ROM checksum error |
| 85 | Data missing alarm |
| 86 | Clock error alarm |
| 88 | Watchdog timeout alarm |
| 90 | Test mode |
| 91 | Load control |
| 92 | Added interval (data correction) |
| 93 | Replaced interval (data correction) |
| 94 | Estimated interval (data correction) |
| 95 | Pulse overflow alarm |
| 96 | Data out of limits |
| 97 | Excluded data |
| 98 | Parity error |
| 99 | Energy type (register changed) |

## TRANS: transaction codes (Appendix A)

| ID | Level | Rule | Source | Violation |
|---|---|---|---|---|
| MDFF-TRANS-001 | MUST | TransCode (500) and PreviousTransCode / CurrentTransCode (550) are one of the codes below. Service orders here are work done by an MP, MDP, ENM or LNSP for a retailer's ServiceOrderRequest or on its own initiative. | App A, p.21 | `X`; `0` (zero). |
| MDFF-TRANS-002 | MUST | `O` is used for Historical Data when the TransCode is unavailable. | App A, p.21; §4.6, p.14; §5.4, p.19-20 | — |
| MDFF-TRANS-003 | — | **Spec issue, no rule:** example I.4 (p.44) sends `550,T,,N,` and says the TransCode `T` is obsolete but "can still be provided for Historical Data". `T` is not in Appendix A and the spec gives no meaning for it, and there is no list of obsolete TransCodes. A reader of historical files may meet it; a writer of current data should not use it. | App I.4, p.44 | — |

| Code | Action | When (paraphrased) |
|---|---|---|
| `A` | Alteration | Any alteration of the metering installation, including replacing one meter with another, all new connections and "Add/Alts" service orders. |
| `C` | Meter Reconfiguration | "Meter Reconfiguration" service order, including off-peak (controlled load) timing changes; not meter removal. |
| `G` | Re-energisation | "Re-energisation" service order. |
| `D` | De-energisation | "De-energisation" service order, including for non-payment. |
| `E` | Estimate | Every Estimate. |
| `N` | Normal Read | Scheduled collection of metering data, with its Substitutions. |
| `O` | Other | "Meter Investigation" and "Miscellaneous" service orders; also Historical Data where the TransCode is unavailable. |
| `S` | Special Read | "Special Read" service order. |
| `R` | Removal of meter | Meter removed, or supply abolished, with no replacement; not a meter changeover or a meter added to an existing configuration (those are alterations). |

## Not specified by the spec

A reader or writer has to choose these; the spec is silent (checked
against the whole text):

- Character encoding, byte order mark, and characters allowed in
  `VarChar` fields (beyond no commas and no `#` in the 100 record).
- Whether blank lines may appear between records.
- The time zone of IntervalDate and every date-time field.
- What `Char(n)` and `VarChar(n)` mean exactly. The usual reading is
  "exactly n characters" and "at most n characters"; the spec uses both
  (e.g. NMI `Char(10)`) without defining them.
- Whether `Numeric(n)` fields (RecordIndicator, IntervalLength,
  StartInterval, EndInterval, ReasonCode) may carry leading zeros or a
  decimal point. (RegisterID, a `VarChar`, appears as both `1` and `001`
  in the examples, p.33-40.)
- The zip's internal layout (number and names of entries).
- Maximum file size or number of records.
- Whether a 400 record may follow a 300 record whose QualityMethod is not
  `V` (see MDFF-400-002).

## Version release history

As printed on p.47 (the cover table on p.4 repeats the 2.7 row). The
VersionHeader in a file (`NEM12`, `NEM13`) names the format, not the
spec version; a file does not say which version of this spec it follows.

| Version | Effective | Change (paraphrased) |
|---|---|---|
| 0.8 | September 2009 | Moved into AEMO format. |
| 1.0 | July 2013 | SGSD change: NSRD becomes an information field only; new AEMO formatting; corrections and clarifications; reason codes removed and added; MSWG feedback. |
| 1.01 | September 2013 | Feedback from the first consultation; clarifications not needing system changes. |
| 1.02 | October 2013 | Feedback from the second consultation. |
| 1.06 | 1 December 2017 | Rules on competition in metering (2015 No. 12), embedded networks (2015 No. 15) and meter replacement (2016 No. 2). |
| 1.07 | 1 October 2021 | NEM Customer Switching. |
| 2.0 | 1 October 2021 | Five Minute Settlement rule (2017 No. 15). |
| 2.1 | 1 October 2021 | AEMO added as a recipient of MDFF. |
| 2.2 | 1 October 2021 | Metering ICF Package 2020: ICF_025, removal of the `N` quality flag. |
| 2.3 | 1 October 2021 | Clarification of the removal of `N`. |
| 2.4 | 1 October 2021 | Consolidates 1.07, 2.0, 2.1, 2.2 and 2.3. |
| 2.5 | 1 May 2022 | Reason codes updated (Retail Electricity October 2021 consultation). |
| 2.6 | 4 November 2024 | Reason codes for the July 2023 REMP consultation: ICF_054, substitution type review. |
| 2.7 | 1 December 2025 | ICF_079, NEM12 MDFF inconsistencies. |
