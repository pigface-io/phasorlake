# MDFF NEM12 and NEM13: record layouts

One table per record type, fields in their exact order. Format and
requirement code are as printed in the spec (v2.7); requirement codes:
`M` mandatory, `R` required if available, `N` not required (§3.3.6, p.8-9;
MDFF-FILE-027). "Rules" lists the IDs in [requirements.md](requirements.md)
that apply to the field. Example lines are the spec's own, joined where
the PDF wraps them; interval lists are shortened with `…` where noted.

Notation, as used by the spec and not defined by it: `Char(n)` reads as
exactly n characters, `VarChar(n)` / `Varchar(n)` as at most n,
`Numeric(n)` as a number of at most n digits, `Numeric(sx.y)` as the
Appendix B number format of the UOM (MDFF-NUM-001), and `Date(8)`,
`DateTime(12)`, `DateTime(14)` as in MDFF-FILE-023 to 025.

Every record: no leading or trailing spaces (MDFF-FILE-018), no commas
inside fields (MDFF-FILE-021), every field present even when null
(MDFF-FILE-020), CRLF line end (MDFF-FILE-017).

## NEM12

Order: `100 ( 200 ( 300 400* 500* )+ )+ 900` (MDFF-BLOCK-001).

### 100 header (5 fields)

Spec example (p.9): `100,NEM12,200301011534,MDP1,Retailer1`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `100` | MDFF-100-001, MDFF-100-007 |
| 2 | VersionHeader | VarChar(5) | M | `NEM12` | MDFF-100-002, MDFF-FILE-006, MDFF-FILE-010 |
| 3 | DateTime | DateTime(12) | M | file creation date-time | MDFF-100-003, MDFF-FILE-024 |
| 4 | FromParticipant | VarChar(10) | M | Participant ID of the generating MDP; no `#` | MDFF-100-004, MDFF-FILE-009 |
| 5 | ToParticipant | VarChar(10) | M | Participant ID of the recipient (Registered Participant, MDP or ENM); no `#` | MDFF-100-005, MDFF-FILE-009 |

### 200 NMI data details (10 fields)

Spec example (p.10): `200,VABD000163,E1Q1,1,E1,N1,METSER123,kWh,30,20040120`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `200` | MDFF-200-001 |
| 2 | NMI | Char(10) | M | NMI without check digit or suffix | MDFF-200-002 |
| 3 | NMIConfiguration | VarChar(240) | M | all NMI suffixes of the NMI, e.g. `E1Q1` | MDFF-200-003, MDFF-200-004 |
| 4 | RegisterID | VarChar(10) | M/N | e.g. `1`, `2`, `E1`, `B1`; M for types 4, 4A, 5 from the Current MDP, N for types 1-3, 7 or to another MDP | MDFF-200-005, MDFF-200-006 |
| 5 | NMISuffix | Char(2) | M | e.g. `E1`, `B1`, `Q1`, `K1` | MDFF-200-007 |
| 6 | MDMDataStreamIdentifier | Char(2) | M/N | e.g. `N1`, `N2`; M if the data is or would be sent to MSATS by the sender | MDFF-200-008 |
| 7 | MeterSerialNumber | VarChar(12) | M/N | N for type 7, logical meters, Historical Data, summated meters | MDFF-200-009 |
| 8 | UOM | VarChar(5) | M | Appendix B, case-insensitive | MDFF-200-010, MDFF-UOM-001, MDFF-UOM-002 |
| 9 | IntervalLength | Numeric(2) | M | `5`, `15`, `30` | MDFF-200-011 |
| 10 | NextScheduledReadDate | Date(8) | M/N | N for remotely read meters and meters not to be read again | MDFF-200-012, MDFF-FILE-023 |

### 300 interval data (2 + N + 5 fields, N = 1440 / IntervalLength)

Spec example (p.11, values elided by the spec): `300,20030501,50.1, . . . ,21.5,V,,,20030101153445,20030102023012`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `300` | MDFF-300-001 |
| 2 | IntervalDate | Date(8) | M | | MDFF-300-002, MDFF-300-017, MDFF-FILE-023 |
| 3 … 2+N | IntervalValue1 … IntervalValueN | Numeric(sx.y) | M | N = 288 / 96 / 48; non-negative; decimals allowed; no exponent; never null; Appendix B format of the UOM | MDFF-300-003 to MDFF-300-007, MDFF-FILE-019, MDFF-NUM-001 |
| 3+N | QualityMethod | VarChar(3) | M | `A`, `V`, or `E`/`F`/`S` + 2-digit method flag | MDFF-300-008 to MDFF-300-012, MDFF-QUAL-001 to MDFF-QUAL-010 |
| 4+N | ReasonCode | Numeric(3) | M/N | Appendix E; M with `S`/`F`; optional with `A`/`E`; empty with `V` | MDFF-300-013, MDFF-REASON-001 to MDFF-REASON-006 |
| 5+N | ReasonDescription | VarChar(240) | N/M | M when ReasonCode is `0` | MDFF-300-014, MDFF-REASON-004 |
| 6+N | UpdateDateTime | DateTime(14) | M/N | MDP version date-time | MDFF-300-015, MDFF-FILE-025 |
| 7+N | MSATSLoadDateTime | DateTime(14) | R | MSATS load date-time | MDFF-300-016, MDFF-FILE-025 |

Field counts: 295 (5-minute), 103 (15-minute), 55 (30-minute) (MDFF-300-018).

### 400 interval event (6 fields)

Spec examples (p.13):

```
400,1,28,S14,32,
400,1,26,A,,
400,27,31,S53,9,
400,32,48,E52,,
```

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `400` | MDFF-400-001, MDFF-400-002 |
| 2 | StartInterval | Numeric(4) | M | 1 … N, ≤ EndInterval | MDFF-400-003 to MDFF-400-005 |
| 3 | EndInterval | Numeric(4) | M | StartInterval … N | MDFF-400-003 to MDFF-400-005 |
| 4 | QualityMethod | VarChar(3) | M | as 300 but never `V` | MDFF-400-006, MDFF-QUAL-009 |
| 5 | ReasonCode | Numeric(3) | M/N | Appendix E; optional with `E` | MDFF-400-007, MDFF-QUAL-006, MDFF-QUAL-008 |
| 6 | ReasonDescription | VarChar(240) | N/M | M when ReasonCode is `0` | MDFF-400-008, MDFF-REASON-004 |

### 500 B2B details (5 fields)

Spec example (p.14): `500,S,RETNSRVCEORD1,20031220154500,001123.5`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `500` | MDFF-500-001 to MDFF-500-004, MDFF-500-011 |
| 2 | TransCode | Char(1) | M | Appendix A: `A`, `C`, `G`, `D`, `E`, `N`, `O`, `S`, `R` | MDFF-500-005, MDFF-TRANS-001, MDFF-TRANS-002 |
| 3 | RetServiceOrder | Varchar(15) | R | service order number | MDFF-500-006 |
| 4 | ReadDateTime | DateTime(14) | M/R/N | R for Historical Data, N for Estimates; time `000001` if unknown | MDFF-500-007, MDFF-500-008 |
| 5 | IndexRead | Varchar(15) | R/N | as displayed on the meter, leading/trailing zeros kept | MDFF-500-009, MDFF-READ-001 to MDFF-READ-006 |

### 900 end (1 field)

Spec example (p.15): `900`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `900` | MDFF-900-001, MDFF-900-002 |

## NEM13

Order: `100 ( 250 550* )+ 900` (MDFF-BLOCK-005).

### 100 header (5 fields)

Spec example (p.16): `100,NEM13,200301011534,MDP1,Retailer1`

As the NEM12 100 record, with VersionHeader `NEM13`.

### 250 accumulation meter data (23 fields)

Spec example (p.16; the PDF wraps it and puts a space before UpdateDateTime, which MDFF-FILE-018 forbids):
`250,1234567890,1141,01,11,11,METSER66,E,000021.2,20031001103230,A,,,000534.5,20040201100030,E64,77,,343.5,kWh,20040509, 20040202125010,20040203000130`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `250` | MDFF-250-001 |
| 2 | NMI | Char(10) | M | NMI without check digit or suffix | MDFF-250-002 |
| 3 | NMIConfiguration | VarChar(240) | M | all NMI suffixes, e.g. `1141` | MDFF-250-003 |
| 4 | RegisterID | VarChar(10) | M | e.g. `1`, `2` | MDFF-250-004 |
| 5 | NMISuffix | Char(2) | M | e.g. `11`, `41` | MDFF-250-005 |
| 6 | MDMDataStreamIdentifier | Char(2) | M/N | e.g. `11`, `41` | MDFF-250-006 |
| 7 | MeterSerialNumber | VarChar(12) | M | | MDFF-250-007 |
| 8 | DirectionIndicator | Char(1) | M | `I` import to grid, `E` export from grid | MDFF-250-008 |
| 9 | PreviousRegisterRead | Varchar(15) | M | dial format, zeros kept, no multiplier | MDFF-250-009, MDFF-250-010, MDFF-READ-007 |
| 10 | PreviousRegisterReadDateTime | DateTime(14) | M | time `000001` if unknown | MDFF-250-011, MDFF-250-026 |
| 11 | PreviousQualityMethod | VarChar(3) | M | `A`, or `F`/`S` + method flag; not `E`, not `V` | MDFF-250-010, MDFF-250-012, MDFF-QUAL-009 |
| 12 | PreviousReasonCode | Numeric(3) | M/N | Appendix E | MDFF-250-013 |
| 13 | PreviousReasonDescription | VarChar(240) | N/M | M when code is `0` | MDFF-250-014 |
| 14 | CurrentRegisterRead | Varchar(15) | M | dial format | MDFF-250-015, MDFF-READ-007 |
| 15 | CurrentRegisterReadDateTime | DateTime(14) | M | Estimates: date ≥ NSRD, time `000000` | MDFF-250-016, MDFF-250-026 |
| 16 | CurrentQualityMethod | VarChar(3) | M | `A`, or `E`/`F`/`S` + method flag; not `V` | MDFF-250-017, MDFF-QUAL-009 |
| 17 | CurrentReasonCode | Numeric(3) | M/N | Appendix E | MDFF-250-018 |
| 18 | CurrentReasonDescription | VarChar(240) | N/M | M when code is `0` | MDFF-250-019 |
| 19 | Quantity | Numeric(sx.y) | M | non-negative, never null, Appendix B format | MDFF-250-020, MDFF-FILE-019, MDFF-NUM-001 |
| 20 | UOM | VarChar(5) | M | Appendix B, case-insensitive | MDFF-250-021, MDFF-UOM-001 |
| 21 | NextScheduledReadDate | Date(8) | M/N | N when the meter will not be read again | MDFF-250-022 |
| 22 | UpdateDateTime | DateTime(14) | M | | MDFF-250-023 |
| 23 | MSATSLoadDateTime | DateTime(14) | R | | MDFF-250-024 |

### 550 B2B details (5 fields)

Spec example (p.19): `550,N,,A,`

| # | Field | Format | Req | Allowed values | Rules |
|---|---|---|---|---|---|
| 1 | RecordIndicator | Numeric(3) | M | `550` | MDFF-550-001 to MDFF-550-003, MDFF-550-009 |
| 2 | PreviousTransCode | Char(1) | M | Appendix A | MDFF-550-004, MDFF-TRANS-001 |
| 3 | PreviousRetServiceOrder | Varchar(15) | R | only to the requesting retailer | MDFF-550-005 |
| 4 | CurrentTransCode | Char(1) | M | Appendix A | MDFF-550-006, MDFF-TRANS-001 |
| 5 | CurrentRetServiceOrder | Varchar(15) | R | only to the requesting retailer | MDFF-550-007 |

### 900 end (1 field)

As in NEM12.

## The spec's full examples

Appendix H (p.33-42) has nine NEM12 files and Appendix I (p.43-45) five
NEM13 ones. They are not copied here. Read them with these known faults,
all in the PDF itself:

| Example | Page | Fault | Breaks |
|---|---|---|---|
| H.3 | p.35 | UpdateDateTime `200312200000001` (15 digits), some with a leading space | MDFF-FILE-025, MDFF-FILE-018 |
| H.7 | p.40 | 52 interval values at IntervalLength 30; 400 records after a 300 whose QualityMethod is `A` | MDFF-300-003, MDFF-400-002 |
| H.8 File 1 | p.40 | spaces before NSRD and UpdateDateTime | MDFF-FILE-018 |
| H.8 File 2 | p.40-41 | 52 interval values at IntervalLength 30; space before MeterSerialNumber | MDFF-300-003, MDFF-FILE-018 |
| §4.5 example | p.13 | `400,61,62.A,89,` (full stop for comma) | MDFF-FILE-020 |
| §5.3 example, I.1-I.4 | p.16, 43-44 | space before UpdateDateTime | MDFF-FILE-018 |
| I.4 | p.44 | TransCode `T`, not in Appendix A | MDFF-TRANS-003 |
| I.5 | p.44 | CurrentRegisterReadDateTime `200311090000000` (15 digits) | MDFF-FILE-025 |
