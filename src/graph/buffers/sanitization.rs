use crate::graph::buffers::events::{ParamEvent, ParamEventType, PluginEvent};
use dropseed_core::plugin::ParamID;
use fnv::FnvHashMap;

/// Sanitizes a plugin's event output stream, by wrapping an event iterator.
///
/// For now, this only means de-duplicating BeginGesture / EndGesture events.
pub struct PluginEventOutputSanitizer {
    is_adjusting_parameter: FnvHashMap<ParamID, bool>,
}

impl PluginEventOutputSanitizer {
    pub fn new(param_capacity: usize) -> Self {
        let mut is_adjusting_parameter = FnvHashMap::default();
        is_adjusting_parameter.reserve(param_capacity * 2);

        Self { is_adjusting_parameter }
    }

    pub fn reset(&mut self) {
        self.is_adjusting_parameter.clear()
    }

    #[inline]
    pub fn sanitize<I>(&mut self, iterator: I) -> ParamOutputSanitizerIter<I>
    where
        I: Iterator<Item = PluginEvent>,
    {
        ParamOutputSanitizerIter { sanitizer: self, iterator }
    }
}

pub struct ParamOutputSanitizerIter<'a, I> {
    sanitizer: &'a mut PluginEventOutputSanitizer,
    iterator: I,
}

impl<'a, I> Iterator for ParamOutputSanitizerIter<'a, I>
where
    I: Iterator<Item = PluginEvent>,
{
    type Item = PluginEvent;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(event) = self.iterator.next() {
            match &event {
                PluginEvent::ParamEvent {
                    cookie: _,
                    event: ParamEvent { parameter_id, event_type, .. },
                } => {
                    let is_adjusting = self
                        .sanitizer
                        .is_adjusting_parameter
                        .entry(ParamID(*parameter_id))
                        .or_insert(false);

                    match event_type {
                        ParamEventType::BeginGesture => {
                            if *is_adjusting {
                                continue;
                            }
                            *is_adjusting = true
                        }
                        ParamEventType::EndGesture => {
                            if !*is_adjusting {
                                continue;
                            }
                            *is_adjusting = false
                        }
                        _ => {}
                    }
                }
                _ => {}
            }

            return Some(event);
        }

        None
    }
}
