use clack_host::utils::Cookie;

use clack_host::events::event_types::NoteEvent as ClackNoteEvent;
use clack_host::events::event_types::*;
use clack_host::events::io::EventBuffer;
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{Event, EventHeader as ClackEventHeader, UnknownEvent};

// Contents of NoteBuffer
#[derive(Copy, Clone)]
pub struct NoteEvent {
    pub header: EventHeader,
    pub channel: i16,
    pub key: i16,
    pub event_type: NoteEventType,
}

// Contents of ParamBuffer
#[derive(Copy, Clone)]
pub struct ParamEvent {
    pub header: EventHeader,
    pub parameter_id: u32,
    pub event_type: ParamEventType,
}

// Contains common data
#[derive(Copy, Clone)]
pub struct EventHeader {
    pub time: u32,
    // TODO: add event flags here when we implement them
}

#[derive(Copy, Clone)]
pub enum NoteEventType {
    On { velocity: f64 },
    Expression { expression_type: NoteExpressionType, value: f64 },
    Choke,
    Off { velocity: f64 },
}

#[derive(Copy, Clone)]
pub enum ParamEventType {
    Value(f64),
    Modulation(f64),
    BeginGesture,
    EndGesture,
}

#[derive(Copy, Clone)]
pub enum PluginEvent {
    NoteEvent { note_port_index: i16, event: NoteEvent },
    ParamEvent { cookie: Cookie },
}

impl PluginEvent {
    pub fn write_to_buffer(&self, buffer: &mut EventBuffer) {
        match self {
            PluginEvent::NoteEvent {
                note_port_index,
                event: NoteEvent { event_type, key, channel, header: EventHeader { time } },
            } => match event_type {
                NoteEventType::On { velocity } => buffer.push(
                    NoteOnEvent(ClackNoteEvent::new(
                        ClackEventHeader::new(*time),
                        -1,
                        *note_port_index,
                        *key,
                        *channel,
                        *velocity,
                    ))
                    .as_unknown(),
                ),
                NoteEventType::Expression { expression_type, value } => buffer.push(
                    NoteExpressionEvent(ClackNoteEvent::new(
                        ClackEventHeader::new(*time),
                        -1,
                        *note_port_index,
                        *key,
                        *channel,
                        *velocity,
                    ))
                    .as_unknown(),
                ),
                NoteEventType::Choke => buffer.push(
                    NoteChokeEvent(ClackNoteEvent::new(
                        ClackEventHeader::new(*time),
                        -1,
                        *note_port_index,
                        *key,
                        *channel,
                        0.0,
                    ))
                    .as_unknown(),
                ),
                NoteEventType::Off { velocity } => buffer.push(
                    NoteOffEvent(ClackNoteEvent::new(
                        ClackEventHeader::new(*time),
                        -1,
                        *note_port_index,
                        *key,
                        *channel,
                        *velocity,
                    ))
                    .as_unknown(),
                ),
            },
            PluginEvent::ParamEvent { .. } => {}
        }
    }
}
