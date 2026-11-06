//! The 10 benchmark tasks (MVP exit criterion, plan Phase 3): a prompt over a local fixture
//! page (`tests/fixtures/sites/bench/`) and a machine check of the outcome.
//!
//! The pages report what the agent did with `benchSubmit(task, payload)` (`bench.js` →
//! `POST /bench/result` on the fixture server). After the agent finishes, a check looks at
//! those payloads and at the browser state read over REST (`tabs.list`).

use serde_json::Value;

/// What a check gets to look at.
pub struct Outcome<'a> {
    /// Payloads the fixture pages reported for this task, oldest first.
    pub results: &'a [Value],
    /// `tabs.list` over REST after the run.
    pub tabs: &'a Value,
}

pub struct Task {
    pub id: &'static str,
    /// What the benchmark exercises.
    pub kind: &'static str,
    /// The prompt; `{base}` becomes `http://127.0.0.1:<port>/bench`.
    pub prompt: &'static str,
    pub check: fn(&Outcome) -> Result<(), String>,
}

/// Said after every prompt.
pub const RULES: &str = "Use only the browsermcp tools; never take screenshots. When you are \
                         finished, reply with one short line saying what you did.";

pub const TASKS: &[Task] = &[
    Task {
        id: "search",
        kind: "search results page",
        prompt: "Go to {base}/search.html, search for \"wireless keyboard\" and add the wireless \
                 keyboard sold by KeyCo to the cart.",
        check: |o| {
            let products: Vec<&str> = o
                .results
                .iter()
                .filter_map(|r| r["product"].as_str())
                .collect();
            expect(!products.is_empty(), "nothing was added to the cart")?;
            expect(
                products.iter().all(|p| *p == "kb-keyco-k7"),
                format!("added {products:?}, expected only kb-keyco-k7"),
            )
        },
    },
    Task {
        id: "wizard",
        kind: "multi-step form wizard",
        prompt: "Go to {base}/wizard.html and create an account: name Dana Reed, email \
                 dana@example.test, the Pro plan billed yearly. Accept the terms and finish \
                 the sign-up.",
        check: |o| {
            let r = last(o)?;
            same(&r["name"], "Dana Reed")?;
            same(&r["email"], "dana@example.test")?;
            same(&r["plan"], "pro")?;
            same(&r["billing"], "yearly")?;
            expect(r["agreed"] == true, "terms not accepted")
        },
    },
    Task {
        id: "pagination",
        kind: "pagination list",
        prompt: "Go to {base}/pagination.html. The catalog has several pages. Find the part with \
                 SKU Q-4417, type its price exactly as shown into the Answer box and submit it.",
        check: |o| {
            let answer = last(o)?["answer"].as_str().unwrap_or_default().to_owned();
            expect(
                answer.contains("63.10"),
                format!("answer {answer:?}, expected $63.10"),
            )
        },
    },
    Task {
        id: "compare",
        kind: "multi-tab compare",
        prompt: "Go to {base}/compare.html. Open each laptop's spec page in its own new tab and \
                 keep all tabs open. Compare their battery life, then on the compare page \
                 choose the laptop with the longer battery life and submit.",
        check: |o| {
            same(&last(o)?["choice"], "borealis-15")?;
            let urls: Vec<&str> = o.tabs["tabs"]
                .as_array()
                .map(|t| t.iter().filter_map(|t| t["url"].as_str()).collect())
                .unwrap_or_default();
            for page in ["laptop-aurora.html", "laptop-borealis.html"] {
                expect(
                    urls.iter().any(|u| u.contains(page)),
                    format!("{page} is not open in a tab (tabs: {urls:?})"),
                )?;
            }
            Ok(())
        },
    },
    Task {
        id: "table",
        kind: "table extraction",
        prompt: "Go to {base}/table.html, add up the Amount of all orders whose status is \
                 Shipped, and submit the total (a number like 123.45) in the report box.",
        check: |o| {
            let text = last(o)?["total"].as_str().unwrap_or_default().to_owned();
            let number: f64 = text
                .replace(['$', ',', ' '], "")
                .parse()
                .unwrap_or(f64::NAN);
            expect(
                (number - 566.0).abs() < 0.005,
                format!("total {text:?}, expected 566.00"),
            )
        },
    },
    Task {
        id: "modal",
        kind: "modal dialog",
        prompt: "Go to {base}/modal.html and delete the project \"Old Site\" (confirm in the \
                 dialog). Do not delete any other project.",
        check: |o| {
            let r = last(o)?;
            same(&r["deleted"], "Old Site")?;
            expect(
                r["remaining"] == serde_json::json!(["Marketing", "Docs"]),
                format!("remaining projects {}", r["remaining"]),
            )
        },
    },
    Task {
        id: "select",
        kind: "dropdowns (native + custom listbox)",
        prompt: "Go to {base}/select.html and place an order: country Ukraine, Express shipping, \
                 quantity 3, color Forest green, with gift wrap.",
        check: |o| {
            let r = last(o)?;
            same(&r["country"], "Ukraine")?;
            same(&r["speed"], "express")?;
            expect(r["quantity"] == 3, format!("quantity {}", r["quantity"]))?;
            same(&r["color"], "Forest green")?;
            expect(r["gift"] == true, "no gift wrap")
        },
    },
    Task {
        id: "scroll",
        kind: "infinite scroll",
        prompt: "Go to {base}/scroll.html. The feed loads more posts as you scroll down. Find \
                 post #42 and like it. Do not like any other post.",
        check: |o| {
            let posts: Vec<i64> = o
                .results
                .iter()
                .filter_map(|r| r["post"].as_i64())
                .collect();
            expect(
                posts == [42],
                format!("liked posts {posts:?}, expected [42]"),
            )
        },
    },
    Task {
        id: "login",
        kind: "login (no iframe)",
        prompt: "Go to {base}/login.html, sign in with username mira and password s3cret-Pass! \
                 and download the October statement.",
        check: |o| same(&last(o)?["user"], "mira"),
    },
    Task {
        id: "recovery",
        kind: "error recovery",
        prompt: "Go to {base}/recovery.html and send a support message as Sam Ortiz with the \
                 message \"Order 1042 arrived damaged\". Make sure it is really sent.",
        check: |o| {
            let r = last(o)?;
            same(&r["name"], "Sam Ortiz")?;
            let message = r["message"].as_str().unwrap_or_default().to_lowercase();
            expect(
                message.contains("1042") && message.contains("damaged"),
                format!("message {:?}", r["message"]),
            )?;
            expect(
                r["attempts"].as_i64().unwrap_or(0) >= 2,
                "sent without a retry?",
            )
        },
    },
];

fn expect(ok: bool, why: impl Into<String>) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why.into()) }
}

/// The last reported payload, or "nothing submitted".
fn last<'a>(o: &'a Outcome) -> Result<&'a Value, String> {
    o.results
        .last()
        .ok_or_else(|| "the page reported nothing (task not completed)".into())
}

/// Case-insensitive, trimmed string comparison.
fn same(value: &Value, expected: &str) -> Result<(), String> {
    let got = value.as_str().unwrap_or_default().trim();
    expect(
        got.eq_ignore_ascii_case(expected),
        format!("got {value}, expected {expected:?}"),
    )
}

/// `all` → every task; `a,b` → those (unknown ids are an error).
pub fn select(spec: &str) -> Result<Vec<&'static Task>, String> {
    if spec == "all" {
        return Ok(TASKS.iter().collect());
    }
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|id| {
            TASKS.iter().find(|t| t.id == id).ok_or_else(|| {
                let ids: Vec<_> = TASKS.iter().map(|t| t.id).collect();
                format!("unknown task '{id}' (tasks: {})", ids.join(", "))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn check(id: &str, results: &[Value], tabs: &Value) -> Result<(), String> {
        let task = TASKS.iter().find(|t| t.id == id).unwrap();
        (task.check)(&Outcome { results, tabs })
    }

    #[test]
    fn ten_tasks_with_fixture_files() {
        assert_eq!(TASKS.len(), 10);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/fixtures/sites/bench");
        for task in TASKS {
            let file = task
                .prompt
                .split("{base}/")
                .nth(1)
                .unwrap()
                .split([',', '.', ' '])
                .next()
                .unwrap();
            assert!(
                root.join(format!("{file}.html")).exists(),
                "{}: {file}.html",
                task.id
            );
        }
        assert_eq!(select("all").unwrap().len(), 10);
        assert_eq!(select("table, scroll").unwrap()[1].id, "scroll");
        assert!(select("nope").is_err());
    }

    #[test]
    fn checks_accept_right_and_reject_wrong_outcomes() {
        let none = json!({ "tabs": [] });
        assert!(check("search", &[json!({ "product": "kb-keyco-k7" })], &none).is_ok());
        assert!(check("search", &[json!({ "product": "kb-typeflow" })], &none).is_err());
        assert!(check("search", &[], &none).is_err());
        assert!(check("table", &[json!({ "total": "$566.00" })], &none).is_ok());
        assert!(check("table", &[json!({ "total": "566,5" })], &none).is_err());
        assert!(check("scroll", &[json!({ "post": 42 })], &none).is_ok());
        assert!(
            check(
                "scroll",
                &[json!({ "post": 41 }), json!({ "post": 42 })],
                &none
            )
            .is_err()
        );

        let compare = [json!({ "choice": "borealis-15" })];
        let open = json!({ "tabs": [
            { "url": "http://x/bench/compare.html" },
            { "url": "http://x/bench/laptop-aurora.html" },
            { "url": "http://x/bench/laptop-borealis.html" },
        ] });
        assert!(check("compare", &compare, &open).is_ok());
        assert!(check("compare", &compare, &none).is_err());
        assert!(
            check(
                "recovery",
                &[json!({ "name": "Sam Ortiz", "message": "Order 1042 arrived damaged", "attempts": 2 })],
                &none
            )
            .is_ok()
        );
    }
}
