//! The registry the NEM kit reads with, against the shapes AEMO publishes
//! under one report pair and the columns it renamed.

use utilake_mms_decode::IRow;
use utilake_registry::{ResolveOptions, Status};

fn irow<'a>(pair: (&'a str, &'a str), version: u16, columns: &'a [&'a str]) -> IRow<'a> {
    IRow {
        report_type: pair.0,
        report_subtype: pair.1,
        version,
        columns,
    }
}

#[test]
fn an_area_report_goes_to_its_area_table_and_a_regional_one_to_the_regions() {
    let r = utilake_nem::registry();
    let opts = ResolveOptions::default();
    let regional = [
        "INTERVAL_DATETIME",
        "REGIONID",
        "POWER",
        "QI",
        "TYPE",
        "LASTCHANGED",
    ];
    let areas = [
        "INTERVAL_DATETIME",
        "AREAID",
        "POWER",
        "QI",
        "TYPE",
        "LASTCHANGED",
    ];
    let region = r
        .resolve(&irow(("ROOFTOP", "ACTUAL"), 2, &regional), opts)
        .unwrap();
    let area = r
        .resolve(&irow(("ROOFTOP", "ACTUAL"), 1, &areas), opts)
        .unwrap();
    assert_eq!(r.table(region.table).name, "ROOFTOP_PV_ACTUAL");
    assert_eq!(r.table(area.table).name, "ROOFTOP_PV_ACTUAL_AREA");
    assert_eq!(area.status, Status::Conforms, "{:?}", area.warnings);
    assert_eq!(r.table(area.table).temporal.entity, ["AREAID", "TYPE"]);
    // The name of a pair is its regional table.
    assert_eq!(
        r.table_by_pair(utilake_registry::Market::Nem, "ROOFTOP", "ACTUAL"),
        Some(region.table)
    );
    let demand = [
        "AREAID",
        "INTERVAL_DATETIME",
        "OPERATIONAL_DEMAND",
        "OPERATIONAL_DEMAND_ADJUSTMENT",
        "WDR_ESTIMATE",
        "LASTCHANGED",
    ];
    let d = r
        .resolve(&irow(("OPERATIONAL_DEMAND", "ACTUAL"), 1, &demand), opts)
        .unwrap();
    assert_eq!(r.table(d.table).name, "DEMANDOPERATIONALACTUAL_AREA");
}

#[test]
fn a_block_under_a_columns_older_name_writes_its_newest() {
    let r = utilake_nem::registry();
    let id = r.table_by_name("MTPASA_REGIONRESULT").unwrap();
    let t = r.table(id);
    let schema = t.schema(0).unwrap();
    let old: Vec<&str> = t.schema_columns(schema);
    assert!(old.contains(&"TOTALSEMISCHEDULEDGEN10"), "{old:?}");
    let res = r
        .resolve(
            &irow(("MTPASA", "REGIONRESULT"), 1, &old),
            ResolveOptions::default(),
        )
        .unwrap();
    assert_eq!(res.status, Status::Conforms, "{:?}", res.warnings);
    let route = res
        .plan
        .routes
        .iter()
        .find(|x| x.source_name == "TOTALSEMISCHEDULEDGEN10")
        .unwrap();
    assert_eq!(route.target_name.as_deref(), Some("TOTALSEMISCHEDULEGEN10"));
    let new = t.columns[t.column_index("TOTALSEMISCHEDULEGEN10").unwrap()]
        .id
        .0;
    assert_eq!(route.target_field_id, Some(new));
    assert_eq!(
        t.column_index("TOTALSEMISCHEDULEDGEN10"),
        t.column_index("TOTALSEMISCHEDULEGEN10")
    );
    // The history the lake publishes keeps each schema's own name.
    let h = t.history();
    assert!(
        h.schema(0)
            .unwrap()
            .by_name("TOTALSEMISCHEDULEDGEN10")
            .is_some()
    );
    assert!(
        h.schema(1)
            .unwrap()
            .by_name("TOTALSEMISCHEDULEGEN10")
            .is_some()
    );
}
