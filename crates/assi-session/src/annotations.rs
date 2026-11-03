//! A bounded, host-ordered shared drawing board. No OS input is generated here.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const MAX_ANNOTATION_POINTS: usize = 128;
pub const MAX_ANNOTATION_STROKES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationPoint {
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationTool {
    Pen,
    Arrow,
    Rectangle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationColor {
    Mint,
    Amber,
    Rose,
    Blue,
    White,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationAuthor {
    Host,
    Viewer,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationStroke {
    pub id: String,
    pub tool: AnnotationTool,
    pub color: AnnotationColor,
    pub width: u8,
    pub points: Vec<AnnotationPoint>,
}
impl AnnotationStroke {
    pub fn validate(&self) -> Result<()> {
        if self.id.len() != 36
            || uuid::Uuid::parse_str(&self.id).is_err()
            || !(1..=12).contains(&self.width)
            || !(1..=MAX_ANNOTATION_POINTS).contains(&self.points.len())
            || (self.tool != AnnotationTool::Pen && self.points.len() != 2)
            || self.points.iter().any(|p| {
                !p.x.is_finite()
                    || !p.y.is_finite()
                    || !(0.0..=1.0).contains(&p.x)
                    || !(0.0..=1.0).contains(&p.y)
            })
        {
            return Err(Error::InvalidAnnotation);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationCommand {
    Add { stroke: AnnotationStroke },
    Undo,
    Clear,
}
impl AnnotationCommand {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Add { stroke } => stroke.validate(),
            _ => Ok(()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedAnnotation {
    #[serde(flatten)]
    pub stroke: AnnotationStroke,
    pub author: AnnotationAuthor,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AnnotationSnapshot {
    pub request_id: Option<String>,
    pub revision: u64,
    pub strokes: Vec<SharedAnnotation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnnotationEvent {
    pub revision: u64,
    pub author: AnnotationAuthor,
    pub command: AnnotationCommand,
}
impl AnnotationSnapshot {
    pub(crate) fn apply(&mut self, event: &AnnotationEvent) -> Result<()> {
        event.command.validate()?;
        if self.revision.checked_add(1) != Some(event.revision) {
            return Err(Error::Protocol);
        }
        match &event.command {
            AnnotationCommand::Add { stroke } => {
                if self
                    .strokes
                    .iter()
                    .any(|existing| existing.stroke.id == stroke.id)
                {
                    return Err(Error::InvalidAnnotation);
                }
                if self.strokes.len() == MAX_ANNOTATION_STROKES {
                    self.strokes.remove(0);
                }
                self.strokes.push(SharedAnnotation {
                    stroke: stroke.clone(),
                    author: event.author,
                });
            }
            AnnotationCommand::Undo => {
                if let Some(index) = self.strokes.iter().rposition(|s| s.author == event.author) {
                    self.strokes.remove(index);
                }
            }
            AnnotationCommand::Clear => self.strokes.clear(),
        }
        self.revision = event.revision;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn add(index: usize) -> AnnotationCommand {
        AnnotationCommand::Add {
            stroke: AnnotationStroke {
                id: uuid::Uuid::from_u128(index as u128).to_string(),
                tool: AnnotationTool::Pen,
                color: AnnotationColor::Mint,
                width: 3,
                points: vec![
                    AnnotationPoint {
                        x: 0.12345678901234567,
                        y: 0.9876543210987654
                    };
                    MAX_ANNOTATION_POINTS
                ],
            },
        }
    }
    #[test]
    fn board_evicts_oldest_and_bounds_each_wire_event() {
        let mut board = AnnotationSnapshot::default();
        for index in 1..=MAX_ANNOTATION_STROKES + 1 {
            let event = AnnotationEvent {
                revision: index as u64,
                author: AnnotationAuthor::Viewer,
                command: add(index),
            };
            let packet = crate::wire::Control::AnnotationEvent {
                event: event.clone(),
            };
            assert!(serde_json::to_vec(&packet).unwrap().len() < 8192);
            board.apply(&event).unwrap();
        }
        assert_eq!(board.strokes.len(), MAX_ANNOTATION_STROKES);
        assert_eq!(
            board.strokes[0].stroke.id,
            uuid::Uuid::from_u128(2).to_string()
        );
    }
    #[test]
    fn duplicate_or_out_of_order_events_do_not_mutate_board() {
        let mut board = AnnotationSnapshot::default();
        let event = AnnotationEvent {
            revision: 1,
            author: AnnotationAuthor::Host,
            command: add(1),
        };
        board.apply(&event).unwrap();
        let old = board.clone();
        assert!(board.apply(&event).is_err());
        assert!(board
            .apply(&AnnotationEvent {
                revision: 2,
                ..event
            })
            .is_err());
        assert_eq!(board, old);
    }
}
