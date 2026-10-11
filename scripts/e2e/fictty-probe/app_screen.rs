//! fictty-probe's screen: thc screen (crates/thc-screen) on the vault `vault`, at the route
//! `{"view", "id"?}`, as `thc screen --view V --id ID` opens it, without a terminal.

pub fn make(plan: &serde_json::Value) -> anyhow::Result<fictty::App> {
    use thc_core::event::Actor;
    use thc_core::vault::{Paths, Vault};
    let paths = Paths::resolve(plan["vault"].as_str().map(std::path::Path::new))?;
    let vault = Vault::open(paths, Actor { kind: "human".into(), name: None }, "screen")?;
    let id = match plan["id"].as_str() {
        Some(i) => Some(vault.store.resolve(i)?),
        None => None,
    };
    let host = thc_screen::ThcHost::new(vault);
    let view = thc_screen::View::parse(plan["view"].as_str().unwrap_or("today")).ok_or_else(|| anyhow::anyhow!("no view {}", plan["view"]))?;
    let route = thc_screen::Route { view, id, query: None, date: None, back: None };
    Ok(host.app(&route, plan["theme"].as_str().unwrap_or("ember-dark")))
}
