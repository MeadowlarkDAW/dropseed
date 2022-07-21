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
    pub plugin_instance_id: u64,
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
    ParamEvent { cookie: Cookie, event: ParamEvent },
}

impl PluginEvent {
    pub fn read_from_clap(
        clap_event: &UnknownEvent,
        target_plugin_instance_id: u64,
    ) -> Option<Self> {
        match clap_event.as_core_event()? {
            CoreEventSpace::NoteOn(NoteOnEvent(e)) => Some(PluginEvent::NoteEvent {
                note_port_index: e.port_index(),
                event: NoteEvent {
                    channel: e.channel(),
                    key: e.key(),
                    header: EventHeader { time: e.header().time() },
                    event_type: NoteEventType::On { velocity: e.velocity() },
                },
            }),
            CoreEventSpace::NoteOff(NoteOffEvent(e)) => Some(PluginEvent::NoteEvent {
                note_port_index: e.port_index(),
                event: NoteEvent {
                    channel: e.channel(),
                    key: e.key(),
                    header: EventHeader { time: e.header().time() },
                    event_type: NoteEventType::Off { velocity: e.velocity() },
                },
            }),
            CoreEventSpace::NoteChoke(NoteChokeEvent(e)) => Some(PluginEvent::NoteEvent {
                note_port_index: e.port_index(),
                event: NoteEvent {
                    channel: e.channel(),
                    key: e.key(),
                    header: EventHeader { time: e.header().time() },
                    event_type: NoteEventType::Choke,
                },
            }),
            CoreEventSpace::NoteExpression(e) => Some(PluginEvent::NoteEvent {
                note_port_index: e.port_index(),
                event: NoteEvent {
                    channel: e.channel(),
                    key: e.key(),
                    header: EventHeader { time: e.header().time() },
                    event_type: NoteEventType::Expression {
                        expression_type: e.expression_type()?,
                        value: e.value(),
                    },
                },
            }),

            CoreEventSpace::ParamValue(e) => Some(PluginEvent::ParamEvent {
                cookie: e.cookie(),
                event: ParamEvent {
                    plugin_instance_id: target_plugin_instance_id,
                    parameter_id: e.param_id(),
                    header: EventHeader { time: e.header().time() },
                    event_type: ParamEventType::Value(e.value()),
                },
            }),
            CoreEventSpace::ParamMod(e) => Some(PluginEvent::ParamEvent {
                cookie: e.cookie(),
                event: ParamEvent {
                    plugin_instance_id: target_plugin_instance_id,
                    parameter_id: e.param_id(),
                    header: EventHeader { time: e.header().time() },
                    event_type: ParamEventType::Modulation(e.value()),
                },
            }),
            CoreEventSpace::ParamGestureBegin(e) => Some(PluginEvent::ParamEvent {
                cookie: Cookie::empty(),
                event: ParamEvent {
                    plugin_instance_id: target_plugin_instance_id,
                    parameter_id: e.param_id(),
                    header: EventHeader { time: e.header().time() },
                    event_type: ParamEventType::BeginGesture,
                },
            }),
            CoreEventSpace::ParamGestureEnd(e) => Some(PluginEvent::ParamEvent {
                cookie: Cookie::empty(),
                event: ParamEvent {
                    plugin_instance_id: target_plugin_instance_id,
                    parameter_id: e.param_id(),
                    header: EventHeader { time: e.header().time() },
                    event_type: ParamEventType::EndGesture,
                },
            }),

            // TODO: handle MIDI events & note end events
            _ => None,
        }
    }

    pub fn write_to_clap_buffer(&self, buffer: &mut EventBuffer) {
        // TODO: Clack event types are a mouthful
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
                    NoteExpressionEvent::new(
                        ClackEventHeader::new(*time),
                        -1,
                        *note_port_index,
                        *key,
                        *channel,
                        *value,
                        *expression_type,
                    )
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
            PluginEvent::ParamEvent {
                cookie,
                event:
                    ParamEvent {
                        header: EventHeader { time },
                        parameter_id,
                        event_type,
                        plugin_instance_id: _,
                    },
            } => match event_type {
                ParamEventType::Value(value) => buffer.push(
                    ParamValueEvent::new(
                        ClackEventHeader::new(*time),
                        *cookie,
                        -1,
                        *parameter_id,
                        -1,
                        -1,
                        -1,
                        *value,
                    )
                    .as_unknown(),
                ),
                ParamEventType::Modulation(modulation_amount) => buffer.push(
                    ParamModEvent::new(
                        ClackEventHeader::new(*time),
                        *cookie,
                        -1,
                        *parameter_id,
                        -1,
                        -1,
                        -1,
                        *modulation_amount,
                    )
                    .as_unknown(),
                ),
                ParamEventType::BeginGesture => buffer.push(
                    ParamGestureBeginEvent::new(ClackEventHeader::new(*time), *parameter_id)
                        .as_unknown(),
                ),
                ParamEventType::EndGesture => buffer.push(
                    ParamGestureEndEvent::new(ClackEventHeader::new(*time), *parameter_id)
                        .as_unknown(),
                ),
            },
        }
    }
}
