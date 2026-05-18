//! Dev utility: dump the .bcc we produce for AuctionContext so it can be
//! eyeball-diffed against Rodin's.

use rossi_build::{Project, ProjectComponent, build};

const AUCTION_CONTEXT_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd;org.eventb.codegen.ui.cgConfig;de.prob.symbolic.ctxBase;de.prob.units.mchBase" version="3">
<org.eventb.core.carrierSet name="_w4LsYO5MEeSpR9iqQeSCVw" org.eventb.core.identifier="USERS"/>
<org.eventb.core.carrierSet name="_qJ3S4O5PEeSpR9iqQeSCVw" org.eventb.core.identifier="AUCTIONS"/>
<org.eventb.core.carrierSet name="_4PKc0O5TEeSpR9iqQeSCVw" org.eventb.core.identifier="ITEMS"/>
</org.eventb.core.contextFile>
"#;

fn main() {
    let pc = ProjectComponent::from_xml("AuctionContext.buc", AUCTION_CONTEXT_BUC).unwrap();
    let project = Project::new("COMP1216", vec![pc]);
    let result = build(&project);
    print!("{}", result.files[0].contents);
}
