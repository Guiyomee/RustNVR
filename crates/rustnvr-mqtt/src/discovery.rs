//! Home Assistant MQTT discovery messages.
//!
//! Each camera becomes a device with: a motion sensor, a connectivity
//! sensor, a recording sensor, a switch for motion detection and a camera
//! entity showing the picture of the last event (with object recognition: a
//! person sensor and a camera entity of the last person, boxes drawn). The
//! NVR itself is a device
//! with the number of connected cameras and the disk usage.
//!
//! Unique identifiers derive from the MQTT client identifier and the camera
//! identifier, so they stay stable across restarts and several NVRs can
//! share a broker.

use crate::{CameraView, Topics};
use rustnvr_core::CameraId;
use serde_json::{Value, json};

/// `(component, object suffix)` of the entities of a camera.
const CAMERA_ENTITIES: &[(&str, &str)] = &[
    ("binary_sensor", "motion"),
    ("binary_sensor", "status"),
    ("binary_sensor", "recording"),
    ("switch", "motion_detection"),
    ("switch", "audio"),
    ("binary_sensor", "person"),
    ("camera", "snapshot"),
    ("camera", "person_snapshot"),
];

/// Discovery node identifier: the client identifier restricted to the
/// characters Home Assistant accepts.
fn node_id(client_id: &str) -> String {
    client_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn config_topic(prefix: &str, component: &str, client_id: &str, object: &str) -> String {
    format!(
        "{prefix}/{component}/{}/{object}/config",
        node_id(client_id)
    )
}

fn nvr_device(client_id: &str) -> Value {
    json!({
        "identifiers": [node_id(client_id)],
        "name": "RustNVR",
        "manufacturer": "RustNVR",
        "model": "NVR",
        "sw_version": env!("CARGO_PKG_VERSION"),
    })
}

/// Discovery topics of a camera (to remove it).
pub(crate) fn camera_config_topics(prefix: &str, client_id: &str, cam: &CameraId) -> Vec<String> {
    CAMERA_ENTITIES
        .iter()
        .map(|(component, what)| {
            config_topic(prefix, component, client_id, &format!("{cam}_{what}"))
        })
        .collect()
}

/// Discovery messages of a camera: `(topic, JSON payload)`.
pub(crate) fn camera_configs(
    prefix: &str,
    client_id: &str,
    topics: &Topics,
    c: &CameraView,
) -> Vec<(String, String)> {
    let node = node_id(client_id);
    let device = json!({
        "identifiers": [format!("{node}_{}", c.id)],
        "name": c.name,
        "manufacturer": "RustNVR",
        "model": "Caméra",
        "via_device": node,
    });
    let availability = json!([{ "topic": topics.availability() }]);
    let entity = |what: &str, name: &str| {
        json!({
            "name": name,
            "unique_id": format!("{node}_{}_{what}", c.id),
            "object_id": format!("{node}_{}_{what}", c.id),
            "device": device,
            "availability": availability,
        })
    };
    let mut out = Vec::new();
    let mut push = |component: &str, what: &str, mut v: Value, extra: Value| {
        if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            v.extend(extra.clone());
        }
        out.push((
            config_topic(prefix, component, client_id, &format!("{}_{what}", c.id)),
            // An empty payload removes the entity.
            if v.is_null() {
                String::new()
            } else {
                v.to_string()
            },
        ));
    };
    push(
        "binary_sensor",
        "motion",
        entity("motion", "Mouvement"),
        json!({
            "device_class": "motion",
            "state_topic": topics.camera(&c.id, "motion"),
            "payload_on": "ON",
            "payload_off": "OFF",
        }),
    );
    push(
        "binary_sensor",
        "status",
        entity("status", "Connexion"),
        json!({
            "device_class": "connectivity",
            "entity_category": "diagnostic",
            "state_topic": topics.camera(&c.id, "status"),
            "payload_on": "online",
            "payload_off": "offline",
        }),
    );
    push(
        "binary_sensor",
        "recording",
        entity("recording", "Enregistrement continu"),
        json!({
            "icon": "mdi:record-rec",
            "state_topic": topics.camera(&c.id, "recording"),
            "payload_on": "ON",
            "payload_off": "OFF",
        }),
    );
    push(
        "switch",
        "motion_detection",
        entity("motion_detection", "Détection de mouvement"),
        json!({
            "icon": "mdi:motion-sensor",
            "entity_category": "config",
            "state_topic": topics.camera(&c.id, "motion_detection"),
            "command_topic": topics.camera(&c.id, "motion_detection/set"),
            "payload_on": "ON",
            "payload_off": "OFF",
        }),
    );
    push(
        "switch",
        "audio",
        entity("audio", "Enregistrement du son"),
        json!({
            "icon": "mdi:microphone",
            "entity_category": "config",
            "state_topic": topics.camera(&c.id, "audio"),
            "command_topic": topics.camera(&c.id, "audio/set"),
            "payload_on": "ON",
            "payload_off": "OFF",
        }),
    );
    if c.person_detection {
        push(
            "binary_sensor",
            "person",
            entity("person", "Personne"),
            json!({
                "device_class": "occupancy",
                "icon": "mdi:account",
                "state_topic": topics.camera(&c.id, "person"),
                "payload_on": "ON",
                "payload_off": "OFF",
            }),
        );
    } else {
        // Without object recognition: remove a sensor announced before.
        push("binary_sensor", "person", Value::Null, Value::Null);
    }
    push(
        "camera",
        "snapshot",
        entity("snapshot", "Dernier mouvement"),
        json!({ "topic": topics.camera(&c.id, "snapshot") }),
    );
    if c.person_detection {
        push(
            "camera",
            "person_snapshot",
            entity("person_snapshot", "Dernière personne"),
            json!({ "topic": topics.camera(&c.id, "person/snapshot") }),
        );
    } else {
        push("camera", "person_snapshot", Value::Null, Value::Null);
    }
    out
}

/// Discovery messages of the NVR device.
pub(crate) fn nvr_configs(prefix: &str, client_id: &str, topics: &Topics) -> Vec<(String, String)> {
    let node = node_id(client_id);
    let availability = json!([{ "topic": topics.availability() }]);
    vec![
        (
            config_topic(prefix, "sensor", client_id, "cameras_online"),
            json!({
                "name": "Caméras en ligne",
                "unique_id": format!("{node}_cameras_online"),
                "object_id": format!("{node}_cameras_online"),
                "icon": "mdi:cctv",
                "state_class": "measurement",
                "state_topic": topics.system("cameras_online"),
                "device": nvr_device(client_id),
                "availability": availability,
            })
            .to_string(),
        ),
        (
            config_topic(prefix, "binary_sensor", client_id, "disk_low"),
            json!({
                "name": "Espace disque faible",
                "unique_id": format!("{node}_disk_low"),
                "object_id": format!("{node}_disk_low"),
                "device_class": "problem",
                "state_topic": topics.system("disk_low"),
                "payload_on": "ON",
                "payload_off": "OFF",
                "device": nvr_device(client_id),
                "availability": availability,
            })
            .to_string(),
        ),
        (
            config_topic(prefix, "binary_sensor", client_id, "storage_problem"),
            json!({
                "name": "Stockage dégradé",
                "unique_id": format!("{node}_storage_problem"),
                "object_id": format!("{node}_storage_problem"),
                "device_class": "problem",
                "state_topic": topics.system("storage_problem"),
                "payload_on": "ON",
                "payload_off": "OFF",
                "device": nvr_device(client_id),
                "availability": availability,
            })
            .to_string(),
        ),
        (
            config_topic(prefix, "sensor", client_id, "disk_used_percent"),
            json!({
                "name": "Disque des enregistrements",
                "unique_id": format!("{node}_disk_used_percent"),
                "object_id": format!("{node}_disk_used_percent"),
                "icon": "mdi:harddisk",
                "unit_of_measurement": "%",
                "state_class": "measurement",
                "entity_category": "diagnostic",
                "state_topic": topics.system("disk_used_percent"),
                "device": nvr_device(client_id),
                "availability": availability,
            })
            .to_string(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_discovery_is_stable_and_removable() {
        let topics = Topics {
            prefix: "rustnvr".into(),
        };
        let cam = CameraView {
            id: CameraId::new("jardin").expect("id"),
            name: "Jardin".into(),
            online: true,
            recording: false,
            motion_enabled: true,
            motion_active: false,
            audio_enabled: true,
            person_detection: true,
        };
        let configs = camera_configs("homeassistant", "rustnvr", &topics, &cam);
        assert_eq!(configs.len(), CAMERA_ENTITIES.len());
        let (topic, payload) = &configs[0];
        assert_eq!(
            topic,
            "homeassistant/binary_sensor/rustnvr/jardin_motion/config"
        );
        let v: Value = serde_json::from_str(payload).expect("json");
        assert_eq!(v["unique_id"], "rustnvr_jardin_motion");
        assert_eq!(v["state_topic"], "rustnvr/jardin/motion");
        assert_eq!(v["device"]["name"], "Jardin");
        assert_eq!(v["availability"][0]["topic"], "rustnvr/availability");
        let switch: Value = serde_json::from_str(&configs[3].1).expect("json");
        assert_eq!(
            switch["command_topic"],
            "rustnvr/jardin/motion_detection/set"
        );
        let audio: Value = serde_json::from_str(&configs[4].1).expect("json");
        assert_eq!(audio["command_topic"], "rustnvr/jardin/audio/set");
        assert_eq!(audio["unique_id"], "rustnvr_jardin_audio");
        let person: Value = serde_json::from_str(&configs[5].1).expect("json");
        assert_eq!(person["state_topic"], "rustnvr/jardin/person");
        assert_eq!(person["device_class"], "occupancy");
        let picture: Value = serde_json::from_str(&configs[7].1).expect("json");
        assert_eq!(picture["topic"], "rustnvr/jardin/person/snapshot");
        assert_eq!(picture["unique_id"], "rustnvr_jardin_person_snapshot");
        // Without object recognition, the sensor is removed (empty payload).
        let without = CameraView {
            person_detection: false,
            ..cam.clone()
        };
        let configs = camera_configs("homeassistant", "rustnvr", &topics, &without);
        assert_eq!(
            configs[5].0,
            "homeassistant/binary_sensor/rustnvr/jardin_person/config"
        );
        assert!(configs[5].1.is_empty());
        assert!(configs[7].1.is_empty());
        // Removal targets exactly the published topics.
        let removal = camera_config_topics("homeassistant", "rustnvr", &cam.id);
        assert_eq!(
            removal,
            configs.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>()
        );
        assert_eq!(node_id("nvr salon/1"), "nvr_salon_1");
    }
}
