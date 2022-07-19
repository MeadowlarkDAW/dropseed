use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{Event, UnknownEvent};

pub use clack_host::events::event_types::*;

#[derive(Clone, Copy)]
pub enum ProcEvent {
    NoteOn(NoteOnEvent),
    NoteOff(NoteOffEvent),
    NoteChoke(NoteChokeEvent),
    NoteEnd(NoteEndEvent),
    NoteExpression(NoteExpressionEvent),
    ParamValue(ParamValueEvent, Option<u64>),
    ParamMod(ParamModEvent, Option<u64>),
    ParamGestureBegin(ParamGestureBeginEvent),
    ParamGestureEnd(ParamGestureEndEvent),
    Transport(TransportEvent),
    Midi(MidiEvent),
    Midi2(Midi2Event),
    // MidiSysEx(MidiSysExEvent<'a>),
}

impl ProcEvent {
    #[inline]
    pub fn as_unknown(&self) -> &UnknownEvent {
        match self {
            ProcEvent::NoteOn(e) => e.as_unknown(),
            ProcEvent::NoteOff(e) => e.as_unknown(),
            ProcEvent::NoteChoke(e) => e.as_unknown(),
            ProcEvent::NoteEnd(e) => e.as_unknown(),
            ProcEvent::NoteExpression(e) => e.as_unknown(),
            ProcEvent::ParamValue(e, _) => e.as_unknown(),
            ProcEvent::ParamMod(e, _) => e.as_unknown(),
            ProcEvent::Transport(e) => e.as_unknown(),
            ProcEvent::Midi(e) => e.as_unknown(),
            ProcEvent::Midi2(e) => e.as_unknown(),
            ProcEvent::ParamGestureBegin(e) => e.as_unknown(),
            ProcEvent::ParamGestureEnd(e) => e.as_unknown(),
        }
    }

    pub fn from_unknown(event: &UnknownEvent) -> Option<Self> {
        match event.as_core_event()? {
            CoreEventSpace::NoteOn(e) => Some(ProcEvent::NoteOn(*e)),
            CoreEventSpace::NoteOff(e) => Some(ProcEvent::NoteOff(*e)),
            CoreEventSpace::NoteChoke(e) => Some(ProcEvent::NoteChoke(*e)),
            CoreEventSpace::NoteEnd(e) => Some(ProcEvent::NoteEnd(*e)),
            CoreEventSpace::NoteExpression(e) => Some(ProcEvent::NoteExpression(*e)),
            CoreEventSpace::ParamValue(e) => Some(ProcEvent::ParamValue(*e, None)),
            CoreEventSpace::ParamMod(e) => Some(ProcEvent::ParamMod(*e, None)),
            CoreEventSpace::ParamGestureBegin(e) => Some(ProcEvent::ParamGestureBegin(*e)),
            CoreEventSpace::ParamGestureEnd(e) => Some(ProcEvent::ParamGestureEnd(*e)),
            CoreEventSpace::Transport(e) => Some(ProcEvent::Transport(*e)),
            CoreEventSpace::Midi(e) => Some(ProcEvent::Midi(*e)),
            CoreEventSpace::Midi2(e) => Some(ProcEvent::Midi2(*e)),
            CoreEventSpace::MidiSysEx(_) => None, // TODO
        }
    }
}

pub trait CoreEventExt {}

impl<'a> CoreEventExt for CoreEventSpace<'a> {}
