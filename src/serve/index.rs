//! The index page. Rendered from the catalogue so it always matches what is
//! actually being served.

use crate::fixtures::Fixture;

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

pub fn render(fixtures: &[Fixture], base: &str) -> String {
    let rows: String = fixtures
        .iter()
        .map(|fixture| {
            format!(
                r#"      <article>
        <h2>{title}</h2>
        <p class="kind">{kind}</p>
        <p>{purpose}</p>
        <p><a href="/{route}"><code>{base}/{route}</code></a></p>
      </article>
"#,
                title = escape(fixture.title),
                kind = escape(fixture.delivery.label()),
                purpose = escape(fixture.purpose),
                route = escape(fixture.route),
                base = escape(base),
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

    #[test]
    fn lists_every_fixture() {
        let fixtures = catalogue();
        let html = render(&fixtures, "http://localhost:8080");
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
    fn escapes_markup_in_text() {
        assert_eq!(escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
