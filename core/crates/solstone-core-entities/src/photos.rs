// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-side face presentation shared by the owner's pages and connected tools.
//! Private archives and owner state never form part of a response.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use solstone_core_entity::faceprints as fp;
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    EntityNotFound,
    KeyNotFound,
    Disabled,
    Action(fp::ActionError),
    Read(String),
}
impl From<String> for Error {
    fn from(error: String) -> Self {
        Self::Read(error)
    }
}
impl From<fp::ActionError> for Error {
    fn from(error: fp::ActionError) -> Self {
        Self::Action(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    None,
    Possible,
    Suggested,
    Confirmed,
    Dismissed,
}
impl From<fp::panel::State> for State {
    fn from(state: fp::panel::State) -> Self {
        match state {
            fp::panel::State::Hidden => Self::None,
            fp::panel::State::Possible => Self::Possible,
            fp::panel::State::Suggested => Self::Suggested,
            fp::panel::State::Confirmed => Self::Confirmed,
            fp::panel::State::Dismissed => Self::Dismissed,
        }
    }
}

#[derive(Serialize)]
pub struct Face {
    pub key: fp::FaceKey,
    pub crop_url: String,
    pub source: fp::Source,
    pub day: String,
    pub category: String,
    pub quality: f32,
}
#[derive(Serialize)]
pub struct Panel {
    pub state: State,
    pub faces: Vec<Face>,
    pub default: Option<fp::FaceKey>,
    pub looks: usize,
    pub last_action: Option<String>,
}
pub struct Photo {
    pub state: State,
    pub jpeg: Option<Vec<u8>>,
}

/// Resolve either a durable id or the folder used by the owner's entity page.
pub(crate) fn entity_id(journal: &Path, entity: &str) -> Result<String, Error> {
    let map =
        solstone_core_entity::read_identity_map(journal).map_err(|e| Error::Read(e.to_string()))?;
    if map.resolved.contains_key(entity) {
        return Ok(entity.to_owned());
    }
    let identity = solstone_core_entity::read_entity_identity(journal, entity)
        .map_err(|e| Error::Read(e.to_string()))?
        .ok_or(Error::EntityNotFound)?;
    let id = identity.entity_id();
    map.resolved
        .contains_key(id)
        .then(|| id.to_owned())
        .ok_or(Error::EntityNotFound)
}

struct Context {
    id: String,
    archive: Option<fp::Archive>,
    state: fp::FacesState,
    panel: fp::panel::Panel,
}

/// The only panel computation: every detail, image and MCP read uses it.
fn context(journal: &Path, entity: &str) -> Result<Context, Error> {
    let id = entity_id(journal, entity)?;
    let (archive, state) = fp::load(journal, &id)?;
    let principal = solstone_core_entity::read_journal_principal(journal)
        .map_err(|e| Error::Read(e.to_string()))?;
    let principal_id = principal
        .as_ref()
        .and_then(|p| p.get("id"))
        .and_then(serde_json::Value::as_str);
    let owner = if let Some(owner) = principal_id.filter(|owner| *owner != id) {
        fp::load(journal, owner)?
            .0
            .and_then(|a| fp::panel::owner_centroid(&a.faces))
    } else {
        None
    };
    let faces = archive
        .as_ref()
        .map(|a| a.faces.as_slice())
        .unwrap_or_default();
    let cross = fp::cross::for_entity(journal, &id)?;
    let panel = fp::panel::panel(
        faces,
        &state,
        principal_id == Some(id.as_str()),
        owner.as_deref(),
        Some(cross),
    );
    Ok(Context {
        id,
        archive,
        state,
        panel,
    })
}

fn face_for_key<'a>(context: &'a Context, key: &fp::FaceKey) -> Option<&'a fp::Face> {
    context
        .state
        .decision
        .photo
        .as_ref()
        .map(|p| &p.face)
        .filter(|f| &f.metadata.key == key)
        .or_else(|| {
            context
                .archive
                .as_ref()?
                .faces
                .iter()
                .find(|f| &f.metadata.key == key)
        })
}

pub fn encode_key(key: &fp::FaceKey) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(key).expect("face key serializes"))
}
pub(crate) fn decode_key(key: &str) -> Result<fp::FaceKey, Error> {
    if key.len() > 4096 {
        return Err(Error::KeyNotFound);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(key)
        .map_err(|_| Error::KeyNotFound)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::KeyNotFound)
}

pub fn panel(journal: &Path, entity: &str) -> Result<Panel, Error> {
    let _trust = solstone_core_entity::hold_entity_trust_lock(journal)
        .map_err(|e| Error::Read(e.to_string()))?;
    entity_id(journal, entity)?;
    if !fp::enabled(journal)? {
        return Ok(Panel {
            state: State::None,
            faces: vec![],
            default: None,
            looks: 0,
            last_action: None,
        });
    }
    let c = context(journal, entity)?;
    let faces = c
        .panel
        .faces
        .iter()
        .filter_map(|key| face_for_key(&c, key))
        .filter(|f| f.metadata.crop.is_some())
        .map(|f| Face {
            key: f.metadata.key.clone(),
            crop_url: format!(
                "/app/entities/api/journal/entity/{}/faces/crop/{}",
                c.id,
                encode_key(&f.metadata.key)
            ),
            source: f.metadata.source.clone(),
            day: f.metadata.key.day.clone(),
            category: f.metadata.category.clone(),
            quality: f.metadata.quality,
        })
        .collect();
    Ok(Panel {
        state: c.panel.state.into(),
        faces,
        default: c.panel.default,
        looks: c.panel.looks,
        last_action: c.state.last.map(|u| u.token),
    })
}

fn read_crop(journal: &Path, face: &fp::Face) -> Result<Vec<u8>, Error> {
    let crop = face.metadata.crop.as_deref().ok_or(Error::KeyNotFound)?;
    let path = fp::crop_path(journal, &face.metadata.key, crop)?;
    solstone_core_journal_io::read_bytes(path, vec![]).map_err(|e| Error::Read(e.to_string()))
}

pub fn crop(journal: &Path, entity: &str, key: &fp::FaceKey) -> Result<Vec<u8>, Error> {
    let _trust = solstone_core_entity::hold_entity_trust_lock(journal)
        .map_err(|e| Error::Read(e.to_string()))?;
    entity_id(journal, entity)?;
    if !fp::enabled(journal)? {
        return Err(Error::KeyNotFound);
    }
    let c = context(journal, entity)?;
    // A client may fetch only a currently offered crop, or the chosen photo.
    if !c.panel.faces.contains(key) && c.panel.default.as_ref() != Some(key) {
        return Err(Error::KeyNotFound);
    }
    if let Some(photo) = c
        .state
        .decision
        .photo
        .as_ref()
        .filter(|p| &p.face.metadata.key == key)
    {
        return Ok(photo.jpeg.clone());
    }
    read_crop(journal, face_for_key(&c, key).ok_or(Error::KeyNotFound)?)
}

fn in_facet(face: &fp::Face, facet: &str) -> bool {
    face.metadata.activities.iter().any(|coordinate| {
        serde_json::from_str::<[String; 3]>(coordinate).is_ok_and(|c| c[0] == facet)
    })
}

/// State is journal-level; image bytes require the chosen source in this facet.
pub fn photo(journal: &Path, entity: &str, facet: Option<&str>) -> Result<Photo, Error> {
    let _trust = solstone_core_entity::hold_entity_trust_lock(journal)
        .map_err(|e| Error::Read(e.to_string()))?;
    entity_id(journal, entity)?;
    if !fp::enabled(journal)? {
        return Ok(Photo {
            state: State::None,
            jpeg: None,
        });
    }
    let c = context(journal, entity)?;
    let state = c.panel.state.into();
    if c.state.decision.photo.is_none() {
        let _ = fp::confirmed_photo(journal, &c.id)?;
    }
    let Some(key) = c.panel.default.as_ref() else {
        return Ok(Photo { state, jpeg: None });
    };
    let face = face_for_key(&c, key).ok_or(Error::KeyNotFound)?;
    if facet.is_some_and(|facet| !in_facet(face, facet)) {
        return Ok(Photo { state, jpeg: None });
    }
    let jpeg = if state == State::Confirmed {
        fp::confirmed_photo(journal, &c.id)?
    } else {
        Some(read_crop(journal, face)?)
    };
    Ok(Photo { state, jpeg })
}

/// Owner lists use one file check per resolved folder, never a panel calculation.
pub fn has_photo_in_dir(journal: &Path, folder: &str, enabled: bool) -> bool {
    enabled
        && solstone_core_entity::entity_identity_path(journal, folder)
            .ok()
            .and_then(|p| p.parent().map(|p| p.join(fp::PHOTO)))
            .is_some_and(|p| p.is_file())
}

/// Connected lists additionally admit the confirmed source face in their facet.
pub fn has_photo_in_facet(journal: &Path, folder: &str, facet: &str) -> Result<bool, Error> {
    if !has_photo_in_dir(journal, folder, fp::enabled(journal)?) {
        return Ok(false);
    }
    let (_, state) = fp::load_in_dir(journal, folder)?;
    Ok(state
        .decision
        .photo
        .as_ref()
        .is_some_and(|p| in_facet(&p.face, facet)))
}

/// An unreadable switch withholds photos; other entity fields remain readable.
pub(crate) fn enabled_for_lists(journal: &Path) -> bool {
    fp::enabled(journal).unwrap_or_else(|error| {
        log::warn!("photo suggestions switch unreadable: {error}");
        false
    })
}
