//! Which APN the internet session uses: the device's override, else
//! mobile-broadband-provider-info for the SIM's home network, else the modem's
//! default profile (an empty APN).

use std::fs;

pub const PROVIDERS: &str = "/usr/share/mobile-broadband-provider-info/serviceproviders.xml";

/// The internet APN for a home network in the provider database.
pub fn from_providers(xml: &str, mcc: u16, mnc: u16) -> Option<String> {
    // The real file names its DTD, which roxmltree refuses by default.
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, options).ok()?;
    let number = |n: roxmltree::Node, attr| n.attribute(attr).and_then(|v| v.parse::<u16>().ok());
    let gsm = doc
        .descendants()
        .filter(|n| n.has_tag_name("gsm"))
        .find(|gsm| {
            gsm.children()
                .filter(|n| n.has_tag_name("network-id"))
                .any(|id| number(id, "mcc") == Some(mcc) && number(id, "mnc") == Some(mnc))
        })?;
    let internet = |apn: &roxmltree::Node| {
        let mut usages = apn.children().filter(|n| n.has_tag_name("usage"));
        usages.clone().next().is_none() || usages.any(|u| u.attribute("type") == Some("internet"))
    };
    gsm.children()
        .filter(|n| n.has_tag_name("apn"))
        .find(internet)
        .and_then(|apn| apn.attribute("value"))
        .map(str::to_owned)
}

pub fn lookup(mcc: u16, mnc: u16) -> Option<String> {
    from_providers(&fs::read_to_string(PROVIDERS).ok()?, mcc, mnc)
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
</serviceproviders>"#;

    #[test]
    fn picks_the_internet_apn() {
        assert_eq!(from_providers(XML, 222, 50).as_deref(), Some("iliad"));
        assert_eq!(from_providers(XML, 222, 1).as_deref(), Some("plain"));
        assert_eq!(from_providers(XML, 222, 99), None);
    }
}
