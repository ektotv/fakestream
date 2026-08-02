//! The index page. Rendered from the catalogue so it always matches what is
//! actually being served.

use crate::fixtures::Fixture;
use crate::serve::Readiness;
use std::collections::HashMap;

/// Escape the few characters that would otherwise break out of HTML text.
fn escape(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

pub fn render(fixtures: &[Fixture], base: &str, readiness: &HashMap<String, Readiness>) -> String {
    let rows: String = fixtures
        .iter()
        .map(|fixture| {
            let state = readiness
                .get(fixture.id)
                .copied()
                .unwrap_or(Readiness::Waiting);

            let link = match state {
                Readiness::Ready => format!(
                    r#"<p><a href="/{route}"><code>{base}/{route}</code></a></p>"#,
                    route = escape(fixture.route),
                    base = escape(base),
                ),
                Readiness::Building(fraction) => format!(
                    r#"<p class="pending">generating, {percent}%</p>"#,
                    percent = (fraction * 100.0).round() as u32
                ),
                Readiness::Waiting => r#"<p class="pending">queued</p>"#.to_string(),
            };

            format!(
                r#"      <article>
        <h2>{title}</h2>
        <p class="kind">{kind}</p>
        <p>{purpose}</p>
        {link}
      </article>
"#,
                title = escape(fixture.title),
                kind = escape(fixture.delivery.label()),
                purpose = escape(fixture.purpose),
            )
        })
        .collect();

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>fakestream</title>
    <style>
      body {{ font-family: system-ui, sans-serif; max-width: 46rem; margin: 3rem auto; padding: 0 1rem; line-height: 1.5; }}
      article {{ border-top: 1px solid #ddd; padding-top: 1rem; margin-top: 1rem; }}
      h2 {{ margin-bottom: 0.2rem; font-size: 1.1rem; }}
      .kind {{ margin: 0 0 0.5rem; font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; color: #666; }}
      code {{ background: #f4f4f4; padding: 0.15rem 0.35rem; border-radius: 3px; }}
      .pending {{ color: #999; font-style: italic; }}
    </style>
  </head>
  <body>
    <h1>fakestream</h1>
    <p>Synthetic test video, generated from nothing. Point a player at any of
       the URLs below.</p>
{rows}  </body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::catalogue;

    fn all_ready(fixtures: &[Fixture]) -> HashMap<String, Readiness> {
        fixtures
            .iter()
            .map(|fixture| (fixture.id.to_string(), Readiness::Ready))
            .collect()
    }

    #[test]
    fn lists_every_fixture() {
        let fixtures = catalogue();
        let html = render(&fixtures, "http://localhost:8080", &all_ready(&fixtures));

        for fixture in &fixtures {
            assert!(
                html.contains(fixture.route),
                "{} missing from index",
                fixture.id
            );
            assert!(html.contains(fixture.title), "{} title missing", fixture.id);
        }
    }

    #[test]
    fn an_unbuilt_fixture_offers_no_link() {
        // Linking to a file that is not there yet sends a player to a 404 and
        // makes the tool look broken rather than busy.
        let fixtures = catalogue();
        let html = render(&fixtures, "http://localhost:8080", &HashMap::new());

        assert!(html.contains("queued"));
        assert!(
            !html.contains(&format!("http://localhost:8080/{}", fixtures[0].route)),
            "a queued fixture was linked"
        );
    }

    #[test]
    fn a_building_fixture_reports_its_progress() {
        let fixtures = catalogue();
        let mut readiness = HashMap::new();
        readiness.insert(fixtures[0].id.to_string(), Readiness::Building(0.42));

        let html = render(&fixtures, "http://localhost:8080", &readiness);
        assert!(html.contains("generating, 42%"));
    }

    #[test]
    fn escapes_markup_in_text() {
        assert_eq!(escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
