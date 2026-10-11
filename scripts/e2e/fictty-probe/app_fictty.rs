//! fictty-probe's screen: a fictty UI file (`ui`), with an optional `store` and `themes` file.

pub fn make(plan: &serde_json::Value) -> anyhow::Result<fictty::App> {
    let mut app = fictty::App::file(plan["ui"].as_str().expect("plan.ui"));
    if let Some(p) = plan["store"].as_str() {
        app = app.store(p);
    }
    if let Some(p) = plan["themes"].as_str() {
        app = app.themes_file(std::path::Path::new(p))?;
    }
    Ok(app)
}
