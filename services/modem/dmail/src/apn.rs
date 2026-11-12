//! Which APN the internet session uses: the device's override, else
//! mobile-broadband-provider-info for the SIM's home network, else the modem's
//! default profile (an empty APN). Brands that share a network code, such as
//! Public Mobile on Telus's 302 220, are told apart by the provider name the
//! SIM holds.

use std::fs;

pub const PROVIDERS: &str = "/usr/share/mobile-broadband-provider-info/serviceproviders.xml";

/// The internet APN for a home network in the provider database: the
/// provider named `spn` among those on the network, else the first.
pub fn from_providers(xml: &str, mcc: u16, mnc: u16, spn: Option<&str>) -> Option<String> {
    // The real file names its DTD, which roxmltree refuses by default.
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, options).ok()?;
    let number = |n: roxmltree::Node, attr| n.attribute(attr).and_then(|v| v.parse::<u16>().ok());
    let on_network = |gsm: &roxmltree::Node| {
        gsm.children()
            .filter(|n| n.has_tag_name("network-id"))
            .any(|id| number(id, "mcc") == Some(mcc) && number(id, "mnc") == Some(mnc))
    };
    let internet = |apn: &roxmltree::Node| {
        let mut usages = apn.children().filter(|n| n.has_tag_name("usage"));
        usages.clone().next().is_none() || usages.any(|u| u.attribute("type") == Some("internet"))
    };
    let candidates: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("gsm"))
        .filter(on_network)
        .filter_map(|gsm| {
            let apn = gsm
                .children()
                .filter(|n| n.has_tag_name("apn"))
                .find(internet)?
                .attribute("value")?;
            Some((gsm.parent()?, apn))
        })
        .collect();
    let spn = spn.map(key);
    let named = |provider: &roxmltree::Node| {
        spn.as_deref().is_some_and(|spn| {
            provider
                .children()
                .filter(|n| n.has_tag_name("name"))
                .filter_map(|n| n.text())
                .any(|name| same_brand(&key(name), spn))
        })
    };
    candidates
        .iter()
        .find(|(provider, _)| named(provider))
        .or(candidates.first())
        .map(|(_, apn)| (*apn).to_owned())
}

/// A name's letters and digits, lower case: a SIM may write "PUBLIC" where
/// the database says "Public Mobile".
fn key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// One name starts with the other: "telus" and "telusmobility".
fn same_brand(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

pub fn lookup(mcc: u16, mnc: u16, spn: Option<&str>) -> Option<String> {
    from_providers(&fs::read_to_string(PROVIDERS).ok()?, mcc, mnc, spn)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding='utf-8'?>
<!DOCTYPE serviceproviders SYSTEM "serviceproviders.2.dtd">
<serviceproviders format="2.0">
  <country code="it">
    <provider>
      <name>Iliad</name>
      <gsm>
        <network-id mcc="222" mnc="50"/>
        <apn value="mms.iliad.it"><usage type="mms"/></apn>
        <apn value="iliad"><usage type="internet"/></apn>
      </gsm>
    </provider>
    <provider>
      <name>Other</name>
      <gsm>
        <network-id mcc="222" mnc="01"/>
        <apn value="plain"/>
      </gsm>
    </provider>
  </country>
  <country code="ca">
    <provider>
      <name>Telus Mobility</name>
      <gsm>
        <network-id mcc="302" mnc="220"/>
        <apn value="isp.telus.com"><usage type="internet"/></apn>
      </gsm>
    </provider>
    <provider>
      <name>Public Mobile</name>
      <gsm>
        <network-id mcc="302" mnc="220"/>
        <apn value="sp.mb.com"><usage type="internet"/></apn>
      </gsm>
    </provider>
  </country>
</serviceproviders>"#;

    #[test]
    fn picks_the_internet_apn() {
        assert_eq!(from_providers(XML, 222, 50, None).as_deref(), Some("iliad"));
        assert_eq!(from_providers(XML, 222, 1, None).as_deref(), Some("plain"));
        assert_eq!(from_providers(XML, 222, 99, None), None);
    }

    #[test]
    fn tells_brands_on_one_network_apart() {
        let apn = |spn| from_providers(XML, 302, 220, spn);
        assert_eq!(apn(Some("Public Mobile")).as_deref(), Some("sp.mb.com"));
        assert_eq!(apn(Some("PUBLIC")).as_deref(), Some("sp.mb.com"));
        assert_eq!(apn(Some("TELUS")).as_deref(), Some("isp.telus.com"));
        assert_eq!(apn(Some("Koodo")).as_deref(), Some("isp.telus.com"));
        assert_eq!(apn(None).as_deref(), Some("isp.telus.com"));
    }
}
