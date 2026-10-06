//! `thc alert`: reminders attached to nodes.

use crate::cli::AlertCmd;
use crate::Ctx;
use anyhow::Result;
use serde_json::{Value, json};
use thc_core::{dates, error::usage, event::{Op, Trigger}};
use crate::{alert_json, resolve_alert, review_cmd};

#[derive(clap::Args, Debug)]
pub struct AlertArgs {
    #[command(subcommand)]
    cmd: AlertCmd,
}

pub const SPEC: crate::registry::Spec = crate::spec!("alert", "Alerts (reminders attached to nodes)", AlertArgs, |ctx: &mut Ctx, a: AlertArgs| alert_cmd(ctx, a.cmd), verbs: |m| match m.subcommand_name() {
    Some("add" | "ack" | "snooze" | "rm") => vec!["alert".into()],
    _ => vec![],
});

fn alert_cmd(ctx: &mut Ctx, a: AlertCmd) -> Result<()> {
    match a {
        AlertCmd::Add { id, at, before, anchor } => {
            let id = ctx.resolve(&id)?;
            let today = ctx.out.today;
            let trigger = match (at, before) {
                (Some(at), _) => {
                    let dv = dates::parse(&at, today)?;
                    let dt = match dv {
                        dates::DateVal::Date(d) => d.and_hms_opt(9, 0, 0).unwrap(),
                        dates::DateVal::DateTime(dt) => dt,
                    };
                    Trigger { at: Some(dt.format("%Y-%m-%dT%H:%M").to_string()), offset: None, anchor: None }
                }
                (None, Some(b)) => {
                    let d = dates::user_duration(&b)?;
                    let mins = -d.num_minutes().abs();
                    if anchor != "due" && anchor != "scheduled" {
                        return Err(usage("--anchor must be due or scheduled"));
                    }
                    Trigger { at: None, offset: Some(format!("{mins}m")), anchor: Some(anchor) }
                }
                (None, None) => Trigger { at: None, offset: Some("0m".into()), anchor: Some(anchor) },
            };
            let r = ctx.write(|b| b.add_alert(&id, trigger))?;
            if let Some((ev, aid)) = r {
                let alert = ctx.store().alerts_where("id=?1", &[&aid])?;
                if ctx.out.json {
                    ctx.out.json(&json!({ "ok": true, "tx": ev.first().map(|e| &e.tx), "alert": alert.first() }));
                } else if let Some(al) = alert.first() {
                    ctx.out.line(format!(
                        "alert {} fires {}",
                        ctx.store().short(&al.id),
                        al.fire_at.clone().unwrap_or("-".into()).replace('T', " ")
                    ));
                }
            }
        }
        AlertCmd::Preview { at } => {
            let now = match at {
                Some(w) => {
                    review_cmd::reject_bare_m(&w)?;
                    match dates::parse(&w, ctx.out.today)? {
                        dates::DateVal::DateTime(dt) => dt,
                        dates::DateVal::Date(d) => d.and_hms_opt(9, 0, 0).unwrap(),
                    }
                }
                None => thc_core::alerts::now(),
            };
            // Ignore this device's fired marks, so a preview shows what firing would say.
            let due = ctx.store().due_alerts_all(now)?;
            let plan = thc_core::alerts::plan(ctx.store(), &due, now);
            if ctx.out.json {
                ctx.out.json(&json!({ "at": now.format("%Y-%m-%dT%H:%M").to_string(), "deliveries": plan }));
            } else if plan.is_empty() {
                ctx.out.line(format!("nothing would fire by {}", now.format("%a %H:%M")));
            } else {
                for d in &plan {
                    match d {
                        thc_core::alerts::Delivery::Single { notification: n } => {
                            ctx.out.line(n.title.clone());
                            ctx.out.line(format!("  {}", n.line1));
                            ctx.out.line(format!("  {}", n.line2));
                        }
                        thc_core::alerts::Delivery::Summary { title, body, .. } => {
                            ctx.out.line(title.clone());
                            ctx.out.line(format!("  {body}"));
                        }
                        thc_core::alerts::Delivery::Silent { alerts } => {
                            let l = ctx.out.dim(&format!("({} more than 12 h late: marked fired without a notification)", alerts.len()));
                            ctx.out.line(l);
                        }
                    }
                }
            }
        }
        AlertCmd::Ls { all } => {
            let alerts = if all {
                ctx.store().alerts_where("deleted=0 ORDER BY fire_at", &[])?
            } else {
                ctx.store().alerts_where("deleted=0 AND state IN ('pending','snoozed') ORDER BY fire_at", &[])?
            };
            if ctx.out.json {
                let items: Vec<Value> = alerts.iter().map(|a| alert_json(ctx.store(), a)).collect();
                ctx.out.json(&json!({ "alerts": items }));
            } else if alerts.is_empty() {
                ctx.out.line("no alerts");
            } else {
                for a in &alerts {
                    let label = ctx.store().node(&a.node)?.map(|n| n.label()).unwrap_or_default();
                    let when = a.fire_at.clone().unwrap_or("-".into()).replace('T', " ");
                    let short = ctx.store().short(&a.id);
                    let state = match ctx.store().alert_state(a).as_str() {
                        "fired" => ctx.out.red("fired"),
                        "snoozed" => format!("snoozed until {}", when.get(11..).unwrap_or(&when)),
                        s => s.to_string(),
                    };
                    let l = format!("{}  {}  {:<8} {}", ctx.out.dim(&format!("{short:<6}")), when, state, label);
                    ctx.out.line(l);
                }
            }
        }
        AlertCmd::Ack { id } => {
            let id = resolve_alert(ctx, &id)?;
            let r = ctx.write(|b| {
                b.ops.push(Op::AlertAck { id: id.clone() });
                Ok(())
            })?;
            if r.is_some() {
                ctx.out.line(if ctx.out.json { json!({"ok": true}).to_string() } else { "acknowledged".into() });
            }
        }
        AlertCmd::Snooze { id, until } => {
            let id = resolve_alert(ctx, &id)?;
            dates::reject_bare_m(&until)?;
            let until = match dates::parse_duration(&until) {
                Ok(d) => (dates::now_local() + d).format("%Y-%m-%dT%H:%M").to_string(),
                Err(_) => dates::parse(&until, ctx.out.today)?.fmt(),
            };
            let u = until.clone();
            let r = ctx.write(|b| {
                b.ops.push(Op::AlertSnooze { id: id.clone(), until: u });
                Ok(())
            })?;
            if r.is_some() {
                ctx.out.line(if ctx.out.json { json!({"ok": true, "until": until}).to_string() } else { format!("snoozed until {}", until.replace('T', " ")) });
            }
        }
        AlertCmd::Rm { id } => {
            let id = resolve_alert(ctx, &id)?;
            let r = ctx.write(|b| {
                b.ops.push(Op::AlertRemove { id: id.clone() });
                Ok(())
            })?;
            if r.is_some() {
                ctx.out.line(if ctx.out.json { json!({"ok": true}).to_string() } else { "alert removed".into() });
            }
        }
    }
    Ok(())
}
