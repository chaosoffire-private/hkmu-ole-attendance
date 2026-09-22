//! Dependency-free extractors for the fixed HTML shapes the login chain uses.
//!
//! Regex is deliberately avoided: the inputs are machine-generated forms and
//! substring scanning is auditable.

/// Read a JavaScript scalar assignment such as `var unid="abc";` or `var n=0;`.
pub fn js_var(html: &str, name: &str) -> Option<String> {
    let mut cursor = 0;
    loop {
        let needle = format!("var {name}");
        let found = html.get(cursor..)?.find(&needle)?;
        let start = cursor.checked_add(found)?;
        let rest = html.get(start.checked_add(needle.len())?..)?;
        let trimmed = rest.trim_start();
        let consumed = rest.len().checked_sub(trimmed.len())?;
        let after_name = start.checked_add(needle.len())?.checked_add(consumed)?;

        let Some(value_start) = trimmed.strip_prefix('=') else {
            cursor = after_name;
            continue;
        };
        let value = value_start.trim_start();
        if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let body = value.get(quote.len_utf8()..)?;
            let end = body.find(quote)?;
            return body.get(..end).map(ToOwned::to_owned);
        }
        let end = value
            .find(|c: char| c == ';' || c == '\n' || c.is_whitespace())
            .unwrap_or(value.len());
        return value.get(..end).map(ToOwned::to_owned);
    }
}

/// Whether a page body is the NAM login interstitial rather than real content.
///
/// When OLE stops accepting a session it answers a page request with a small
/// auto-submitting form that bounces the client into the SSO flow. Detecting
/// that shape is what lets a revoked session be told apart from a page that
/// genuinely has no activity on it.
pub fn is_login_redirect(body: &str) -> bool {
    body.len() < 2_000
        && body.contains("/nidp/idff/sso")
        && body.contains("document.forms[0].submit()")
        && !body.contains("take_url")
}

/// Extract the `action` attribute of the first `<form ...>` element.
pub fn first_form_action(html: &str) -> Option<String> {
    let start = html.find("<form")?;
    let rest = html.get(start..)?;
    let tag_end = rest.find('>')?;
    let tag = rest.get(..tag_end)?;
    attribute_value(tag, "action")
}

/// Extract the `action` of the first `<form ...>` element matching `marker`.
pub fn form_action_containing(html: &str, marker: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(found) = html.get(cursor..)?.find("<form") {
        let start = cursor.checked_add(found)?;
        let rest = html.get(start..)?;
        let tag_end = rest.find('>')?;
        let tag = rest.get(..tag_end)?;
        if tag.contains(marker) {
            return attribute_value(tag, "action");
        }
        cursor = start.checked_add(tag_end)?;
    }
    None
}

/// Extract the `value` attribute of the input element named `name`.
///
/// The enclosing tag is located first, then its `value` attribute is read, so
/// the attribute order inside the tag does not matter and a neighbouring
/// element's value can never be picked up.
pub fn input_value_for(html: &str, name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    let name_at = html.find(&needle)?;
    let tag_start = html.get(..name_at)?.rfind('<')?;
    let after_name = name_at.checked_add(needle.len())?;
    let rest = html.get(tag_start..)?;
    let tag_end = rest.find('>')?;
    let tag = rest.get(..tag_end)?;
    if tag_start.checked_add(tag_end)? < after_name {
        return None;
    }
    attribute_value(tag, "value")
}

/// Extract the target of a JavaScript `window.location.href='...'` redirect.
pub fn script_redirect(html: &str) -> Option<String> {
    for marker in [
        "window.location.href='",
        "window.location.href=\"",
        "location.href='",
        "location.href=\"",
    ] {
        if let Some(start) = html.find(marker) {
            let rest = html.get(start.checked_add(marker.len())?..)?;
            let quote = marker.as_bytes().last().copied()?;
            let end = rest.find(char::from(quote))?;
            return rest.get(..end).map(ToOwned::to_owned);
        }
    }
    None
}

/// Read `attr="value"` out of a tag, tolerating unquoted values.
pub fn attribute_value(tag: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=");
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&needle)?;
    let rest = tag.get(at.checked_add(needle.len())?..)?;
    let first = rest.chars().next()?;
    if first == '"' || first == '\'' {
        let body = rest.get(first.len_utf8()..)?;
        let end = body.find(first)?;
        return body.get(..end).map(ToOwned::to_owned);
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>')
        .unwrap_or(rest.len());
    rest.get(..end).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        first_form_action, form_action_containing, input_value_for, is_login_redirect, js_var,
        script_redirect,
    };

    #[test]
    fn extracts_nam_auto_submit_form_action() {
        // Given the NAM interstitial page served mid-login.
        let html = r#"<form method="POST" action="/nidp/idff/sso?id=7&sid=0&option=credential&sid=0&target=https%3A%2F%2Fiole.hkmu.edu.hk%2F"></form>"#;

        // When the form action is read.
        // Then the exact target is returned.
        assert_eq!(
            first_form_action(html).as_deref(),
            Some(
                "/nidp/idff/sso?id=7&sid=0&option=credential&sid=0&target=https%3A%2F%2Fiole.hkmu.edu.hk%2F"
            )
        );
    }

    #[test]
    fn extracts_domino_silent_sso_form_action() {
        // Given the Domino bridge page, which is the second form on its page.
        let html = r#"<form name="_DominoForm" action="/names.nsf?nov-ss-ff-silent&mastercdnioleLogin33310&Login" method="post"><input value="C01818DC00000000" name="%%ModDate" type="hidden"></form>"#;

        // When the action is read by marker.
        // Then the bridge endpoint is returned.
        assert_eq!(
            form_action_containing(html, "nov-ss-ff-silent").as_deref(),
            Some("/names.nsf?nov-ss-ff-silent&mastercdnioleLogin33310&Login")
        );
    }

    #[test]
    fn reads_value_declared_before_name() {
        // Given an input whose value attribute precedes its name attribute.
        let html = r#"<input maxlength="256" value="s1234567" name="Username"/>"#;

        // When the value is read.
        // Then the credential is extracted.
        assert_eq!(
            input_value_for(html, "Username").as_deref(),
            Some("s1234567")
        );
    }

    #[test]
    fn reads_value_declared_after_name() {
        // Given an input whose name attribute precedes its value attribute.
        let html = r#"<input type="hidden" name="option" value="credential">"#;

        // When the value is read.
        // Then it is still found.
        assert_eq!(
            input_value_for(html, "option").as_deref(),
            Some("credential")
        );
    }

    #[test]
    fn ignores_a_value_belonging_to_a_neighbouring_input() {
        // Given two inputs where the first has a value and the second does not.
        let html = r#"<input value="WRONG" name="a"><input name="b">"#;

        // When reading the value of the second input.
        // Then the neighbouring value is not stolen.
        assert_eq!(input_value_for(html, "b"), None);
    }

    #[test]
    fn extracts_javascript_redirect_target() {
        // Given the page NAM returns after accepting a password.
        let html = r"<script>window.location.href='https://auth.hkmu.edu.hk:443/nidp/idff/sso?sid=0';</script>";

        // When the redirect target is read.
        // Then the SSO URL is returned.
        assert_eq!(
            script_redirect(html).as_deref(),
            Some("https://auth.hkmu.edu.hk:443/nidp/idff/sso?sid=0")
        );
    }

    #[test]
    fn extracts_string_javascript_variable() {
        // Given the class-activities page preamble.
        let html = r#"<script>var dburl="/course2604/ELEC3050SEF.nsf"; var unid="356D6AE68EED315148258E7900104316"; var attendance_type="2";</script>"#;

        // When the variables are read.
        // Then each value is returned without quotes.
        assert_eq!(
            js_var(html, "unid").as_deref(),
            Some("356D6AE68EED315148258E7900104316")
        );
        assert_eq!(js_var(html, "attendance_type").as_deref(), Some("2"));
        assert_eq!(
            js_var(html, "dburl").as_deref(),
            Some("/course2604/ELEC3050SEF.nsf")
        );
    }

    #[test]
    fn extracts_numeric_javascript_variable() {
        // Given numeric assignments terminated by a semicolon.
        let html = r"<script>var time_left_second=0; var time_to_start_second=12;</script>";

        // When the variables are read.
        // Then the digits are returned.
        assert_eq!(js_var(html, "time_left_second").as_deref(), Some("0"));
        assert_eq!(js_var(html, "time_to_start_second").as_deref(), Some("12"));
    }

    #[test]
    fn skips_an_earlier_occurrence_that_is_not_an_assignment() {
        // Given a page that mentions the name before assigning it.
        let html = r#"<script>// unid= is set later
        var unid="REAL";</script>"#;

        // When the variable is read.
        // Then the real assignment wins over the comment mention.
        assert_eq!(js_var(html, "unid").as_deref(), Some("REAL"));
    }

    #[test]
    fn reads_an_empty_javascript_variable() {
        // Given a variable assigned an empty string, as on the list page.
        let html = r#"<script>var unid=""; var attendance_type="";</script>"#;

        // When read.
        // Then an empty string is returned rather than None, distinguishing
        // "present but empty" from "absent".
        assert_eq!(js_var(html, "unid").as_deref(), Some(""));
        assert_eq!(js_var(html, "missing"), None);
    }

    #[test]
    fn detects_the_login_interstitial_a_revoked_session_produces() {
        // Given the exact bytes OLE returned for a rejected session, captured
        // from the live server: a small auto-submitting form that bounces the
        // client into the SSO flow instead of serving the class page.
        let revoked = r#"








        


<html>
    <head>
        <META HTTP-EQUIV="expires" CONTENT="0">
    </head>
    <body>

        <form method="POST" enctype="application/x-www-form-urlencoded" action="/nidp/idff/sso?id=7&sid=0&option=credential&sid=0&target=https%3A%2F%2Fiole.hkmu.edu.hk%2Fcourse2604%2FELEC3050SEF.nsf%2F%2Fclass_activities_student%3Freadform%26"></form>

        <script language="JavaScript">
            <!--
                document.forms[0].submit();
            -->
        </script>
    </body>
</html>


 
"#;

        // When inspected.
        // Then it is recognised as an expiry, which is what lets the poller
        // discard the cached session and log in again.
        assert!(is_login_redirect(revoked));
    }

    #[test]
    fn does_not_mistake_a_real_activity_page_for_a_login_redirect() {
        // Given a genuine class-activities page, which carries the submit
        // script but no SSO hop.
        let real = r#"<script>
            var take_url = "/course2604/ELEC3050SEF.nsf/class_activities_student?createdocument&puid=ABC";
        </script>"#;

        // When inspected.
        // Then it is not treated as an expiry, so the poll proceeds normally.
        assert!(!is_login_redirect(real));
    }

    #[test]
    fn does_not_flag_a_large_page_that_merely_mentions_the_sso_endpoint() {
        // Given a page big enough to be real content, which happens to include
        // the SSO path and an auto-submit call somewhere in it.
        let mut big = String::from("<html><body><div>");
        big.push_str(&"real content ".repeat(300));
        big.push_str("/nidp/idff/sso");
        big.push_str("</div><script>document.forms[0].submit();</script></body></html>");

        // When inspected.
        // Then the size guard prevents a false positive, because a real
        // timetable page is far larger than the interstitial.
        assert!(!is_login_redirect(&big));
    }

    #[test]
    fn returns_none_for_missing_markers() {
        // Given a page with none of the expected structures.
        let html = "<html><body>nothing here</body></html>";

        // When every extractor runs.
        // Then each reports absence instead of panicking.
        assert!(first_form_action(html).is_none());
        assert!(input_value_for(html, "Username").is_none());
        assert!(script_redirect(html).is_none());
    }
}
