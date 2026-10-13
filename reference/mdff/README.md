# MDFF (NEM12 and NEM13), for reference

The Meter Data File Format is AEMO's CSV format for meter data in the
NEM: MDPs use it to send metering data to other MDPs, ENMs, Registered
Participants and AEMO, as standalone files or as CSV blocks inside other
formats such as aseXML B2B transactions. It comes in two kinds:
**NEM12** carries interval metering data (one value per 5, 15 or 30
minutes), **NEM13** carries accumulated metering data (register reads).

This directory restates AEMO's *Meter Data File Format Specification
NEM12 & NEM13*, **version 2.7, effective 1 December 2025** (approved
31 March 2025), so that a writer, a reader and a validator can be built
from it without the PDF:

<https://www.aemo.com.au/-/media/files/stakeholder_consultation/consultations/nem-consultations/2024/2024-metering-services-review-package-1/final-stage/mdff-specification-nem12-nem13-v27.pdf>

The PDF itself is not kept here. The text is paraphrased; short field-level
wording is quoted where the exact words matter. Where this directory and
the PDF disagree, the PDF wins: report the difference and fix it here.

## What is here

- [`requirements.md`](requirements.md): every normative requirement as an
  item with a stable ID, a level (MUST, SHOULD, MAY), the section and page
  it comes from, and an input that violates it. Also the code tables
  (Appendices A to F), the spec's ambiguities and errors ("Spec issue"),
  what the spec leaves unsaid, and its release history.
- [`records.md`](records.md): one table per record type (NEM12: 100, 200,
  300, 400, 500, 900; NEM13: 100, 250, 550, 900), fields in exact order
  with format, requirement code, allowed values and the IDs that apply;
  the spec's example lines; and the known faults of its full examples.
- [`examples/`](examples/): small files with CRLF line ends.
  - `nem12-5min-actual.csv`: one day of 5-minute data (288 values) on two
    registers, all actual, no 400 or 500 records.
  - `nem12-30min-mixed-quality.csv`: a manually read 30-minute meter over
    three days: an actual day, a mixed day (QualityMethod `V`, four 400
    records covering intervals 1-48: actual, actual with reason 79,
    substituted, estimated; then a 500 record with the index read), and
    an estimated day.
  - `nem13-accumulation.csv`: an actual read with no 550 (both TransCodes
    would be `N`, so none is required), then an estimate to the NSRD with
    its 550.
  - `invalid/`: each file is a valid one with one fault, named for the
    requirement it breaks (`MDFF-400-005-gap.csv`). The one with
    IntervalLength 10 also breaks MDFF-300-003, as its 288 values no
    longer fit; every other file breaks only its named rule.

  The NMIs (`QTST…`), participants and meter serials are made up.
  Delivered files would be named `nem12#<UniqueID>#<From>#<To>.csv`
  (MDFF-FILE-007); these use descriptive names instead.

## Citing a requirement

Cite the ID and the version: "MDFF-300-003 (MDFF v2.7)". IDs are never
renumbered or reused; when a later version of the spec changes a rule,
the item is updated and says from which version, and a rule that goes
away keeps its ID with a note. A validator should report the ID with
each finding, and a test for a rule should carry the ID in its name.

Nothing in nemlake reads this directory; it is reference for people and
agents. The Rust form of the appendices (codes, units, number formats)
is `crates/nemlake-mdff`.
