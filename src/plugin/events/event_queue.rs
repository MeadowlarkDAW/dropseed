use std::ffi::c_void;
use std::mem::MaybeUninit;

use clap_sys::events::clap_event_header as ClapEventHeader;
use clap_sys::events::clap_event_midi as ClapEventMidi;
use clap_sys::events::clap_event_midi2 as ClapEventMidi2;
use clap_sys::events::clap_event_midi_sysex as ClapEventMidiSysex;
use clap_sys::events::clap_event_note as ClapEventNote;
use clap_sys::events::clap_event_note_expression as ClapEventNoteExpression;
use clap_sys::events::clap_event_param_gesture as ClapEventParamGesture;
use clap_sys::events::clap_event_param_mod as ClapEventParamMod;
use clap_sys::events::clap_event_param_value as ClapEventParamValue;
use clap_sys::events::clap_event_transport as ClapEventTransport;

use super::{
    EventMidi, EventMidi2, EventMidiSysex, EventNote, EventNoteExpression, EventParamGesture,
    EventParamMod, EventParamValue, EventTransport, PluginEvent,
};

// TODO: Use an event queue that supports variable sizes for messages to
// save on memory. The majority of events will be about half the size or
// less than the less common maximum-sized event `EventTransport`.

pub struct EventQueue {
    pub(crate) events: Vec<AllocatedEvent>,
}

impl EventQueue {
    pub fn new(capacity: usize) -> Self {
        Self { events: Vec::with_capacity(capacity) }
    }

    #[inline]
    pub fn push(&mut self, event: PluginEvent) {
        if self.events.len() >= self.events.capacity() {
            log::warn!("Event queue has exceeded its capacity. This will cause an allocation on the audio thread.");
        }

        self.events.push(AllocatedEvent::from_event(event));
    }

    pub fn pop(&mut self) -> Option<AllocatedEvent> {
        self.events.pop()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

pub struct AllocatedEvent {
    pub(crate) data: [u8; std::mem::size_of::<EventTransport>()],
}

impl AllocatedEvent {
    pub fn raw_pointer(&self) -> *const ClapEventHeader {
        self.data.as_ptr() as *const ClapEventHeader
    }

    pub fn from_event(mut event: PluginEvent) -> Self {
        let raw_bytes = match &mut event {
            PluginEvent::Note(e) => unsafe {
                // While setting the size value here in each match arm is redundant, for some
                // reason if we don't use the value `e` in each match arm, the
                // `std::slice::from_raw_parts` doesn't work correctly when compiling in
                // release mode. (we get garbage bytes)
                //
                // My guess for why this happens is something to do with how rust optimizes
                // match statements, and it just so happens to work against us in this
                // (albeit fringe) use case. It might be a bug in the rust compiler itself,
                // but who knows.
                e.0.header.size = std::mem::size_of::<ClapEventNote>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventNote as *const u8,
                    std::mem::size_of::<ClapEventNote>(),
                )
            },
            PluginEvent::NoteExpression(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventNoteExpression>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventNoteExpression as *const u8,
                    std::mem::size_of::<ClapEventNoteExpression>(),
                )
            },
            PluginEvent::ParamValue(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventParamValue>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventParamValue as *const u8,
                    std::mem::size_of::<ClapEventParamValue>(),
                )
            },
            PluginEvent::ParamMod(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventParamMod>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventParamMod as *const u8,
                    std::mem::size_of::<ClapEventParamMod>(),
                )
            },
            PluginEvent::ParamGesture(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventParamGesture>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventParamGesture as *const u8,
                    std::mem::size_of::<ClapEventParamGesture>(),
                )
            },
            PluginEvent::Transport(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventTransport>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventTransport as *const u8,
                    std::mem::size_of::<ClapEventTransport>(),
                )
            },
            PluginEvent::Midi(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventMidi>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventMidi as *const u8,
                    std::mem::size_of::<ClapEventMidi>(),
                )
            },
            PluginEvent::MidiSysex(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventMidiSysex>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventMidiSysex as *const u8,
                    std::mem::size_of::<ClapEventMidiSysex>(),
                )
            },
            PluginEvent::Midi2(e) => unsafe {
                e.0.header.size = std::mem::size_of::<ClapEventMidi2>() as u32;

                std::slice::from_raw_parts(
                    &e.0 as *const ClapEventMidi2 as *const u8,
                    std::mem::size_of::<ClapEventMidi2>(),
                )
            },
        };

        debug_assert!(raw_bytes.len() <= std::mem::size_of::<EventTransport>());

        // This is safe because we ensure that only the correct number of bytes
        // will be read via the event.header.size value, which the constructor
        // of each event ensures is correct.
        //let mut data: [u8; std::mem::size_of::<EventTransport>()] =
        //unsafe { MaybeUninit::uninit().assume_init() };
        let mut data: [u8; std::mem::size_of::<EventTransport>()] =
            [0; std::mem::size_of::<EventTransport>()];

        data[0..raw_bytes.len()].copy_from_slice(raw_bytes);

        Self { data }
    }

    pub fn get(&self) -> Result<PluginEvent, ()> {
        Err(())

        /*
        // The event header is always the first bytes in every event.
        let header = unsafe { &*(self.data.as_ptr() as *const ClapEventHeader) };

        match header.type_ {
            clap_sys::events::CLAP_EVENT_NOTE_ON
            | clap_sys::events::CLAP_EVENT_NOTE_OFF
            | clap_sys::events::CLAP_EVENT_NOTE_CHOKE
            | clap_sys::events::CLAP_EVENT_NOTE_END => Ok(PluginEvent::Note(EventNote(unsafe {
                *(self.data.as_ptr() as *const ClapEventNote)
            }))),
            clap_sys::events::CLAP_EVENT_NOTE_EXPRESSION => {
                Ok(PluginEvent::NoteExpression(EventNoteExpression(unsafe {
                    *(self.data.as_ptr() as *const ClapEventNoteExpression)
                })))
            }
            clap_sys::events::CLAP_EVENT_PARAM_VALUE => {
                Ok(PluginEvent::ParamValue(EventParamValue(unsafe {
                    *(self.data.as_ptr() as *const ClapEventParamValue)
                })))
            }
            clap_sys::events::CLAP_EVENT_PARAM_MOD => {
                Ok(PluginEvent::ParamMod(EventParamMod(unsafe {
                    *(self.data.as_ptr() as *const ClapEventParamMod)
                })))
            }
            clap_sys::events::CLAP_EVENT_PARAM_GESTURE_BEGIN
            | clap_sys::events::CLAP_EVENT_PARAM_GESTURE_END => {
                Ok(PluginEvent::ParamGesture(EventParamGesture(unsafe {
                    *(self.data.as_ptr() as *const ClapEventParamGesture)
                })))
            }
            clap_sys::events::CLAP_EVENT_TRANSPORT => {
                Ok(PluginEvent::Transport(EventTransport(unsafe {
                    *(self.data.as_ptr() as *const ClapEventTransport)
                })))
            }
            clap_sys::events::CLAP_EVENT_MIDI => Ok(PluginEvent::Midi(EventMidi(unsafe {
                *(self.data.as_ptr() as *const ClapEventMidi)
            }))),
            clap_sys::events::CLAP_EVENT_MIDI_SYSEX => {
                Ok(PluginEvent::MidiSysex(EventMidiSysex(unsafe {
                    *(self.data.as_ptr() as *const ClapEventMidiSysex)
                })))
            }
            clap_sys::events::CLAP_EVENT_MIDI2 => Ok(PluginEvent::Midi2(EventMidi2(unsafe {
                *(self.data.as_ptr() as *const ClapEventMidi2)
            }))),
            _ => Err(()),
        }
        */
    }
}
