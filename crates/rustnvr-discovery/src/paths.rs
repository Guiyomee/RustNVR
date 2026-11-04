//! Usual RTSP paths of common camera makers, tried when a camera does not
//! answer ONVIF. Each entry gives the main stream and, when the maker has
//! one, the sub stream.

/// RTSP paths of a maker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownPaths {
    /// Makers using these paths.
    pub makers: &'static str,
    /// Main stream path (with its leading `/`).
    pub main: &'static str,
    /// Sub stream path.
    pub sub: Option<&'static str>,
}

/// Most common first.
pub const KNOWN_PATHS: &[KnownPaths] = &[
    KnownPaths {
        makers: "Hikvision, Annke, HiLook",
        main: "/Streaming/Channels/101",
        sub: Some("/Streaming/Channels/102"),
    },
    KnownPaths {
        makers: "Dahua, Amcrest, Imou, Lorex",
        main: "/cam/realmonitor?channel=1&subtype=0",
        sub: Some("/cam/realmonitor?channel=1&subtype=1"),
    },
    KnownPaths {
        makers: "Reolink",
        main: "/h264Preview_01_main",
        sub: Some("/h264Preview_01_sub"),
    },
    KnownPaths {
        makers: "Reolink (H.265)",
        main: "/h265Preview_01_main",
        sub: Some("/h264Preview_01_sub"),
    },
    KnownPaths {
        makers: "TP-Link Tapo, VIGI",
        main: "/stream1",
        sub: Some("/stream2"),
    },
    KnownPaths {
        makers: "Uniview",
        main: "/unicast/c1/s0/live",
        sub: Some("/unicast/c1/s1/live"),
    },
    KnownPaths {
        makers: "Axis",
        main: "/axis-media/media.amp",
        sub: Some("/axis-media/media.amp?resolution=640x360"),
    },
    KnownPaths {
        makers: "Foscam",
        main: "/videoMain",
        sub: Some("/videoSub"),
    },
    KnownPaths {
        makers: "Xiongmai, caméras génériques",
        main: "/user=admin&password=&channel=1&stream=0.sdp",
        sub: Some("/user=admin&password=&channel=1&stream=1.sdp"),
    },
    KnownPaths {
        makers: "Générique",
        main: "/live",
        sub: None,
    },
    KnownPaths {
        makers: "Générique",
        main: "/11",
        sub: Some("/12"),
    },
];

/// The entries to try for a camera whose RTSP `Server` header is `server`:
/// the maker's own paths first when the header names it.
pub fn ordered_for(server: Option<&str>) -> Vec<&'static KnownPaths> {
    let hint = server.unwrap_or_default().to_ascii_lowercase();
    let mut out: Vec<&KnownPaths> = KNOWN_PATHS.iter().collect();
    out.sort_by_key(|p| {
        let named = p
            .makers
            .split([',', ' ', '('])
            .map(|m| m.trim().to_ascii_lowercase())
            .any(|m| m.len() >= 4 && hint.contains(&m));
        !named
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_named_maker_comes_first() {
        assert_eq!(ordered_for(None)[0].main, "/Streaming/Channels/101");
        assert_eq!(
            ordered_for(Some("Rtsp Server/2.0 Reolink"))[0].main,
            "/h264Preview_01_main"
        );
        assert_eq!(
            ordered_for(Some("Dahua Rtsp Server/3.0"))[0].makers,
            "Dahua, Amcrest, Imou, Lorex"
        );
        assert_eq!(ordered_for(Some("something")).len(), KNOWN_PATHS.len());
    }
}
