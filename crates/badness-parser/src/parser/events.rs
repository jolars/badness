//! Flat parser events.
//!
//! The parser emits a flat `Vec<Event>` rather than building the tree directly
//! (the rust-analyzer shape). Tokens are referenced by index into the
//! token stream, and there is deliberately **no** `Error` event — syntax errors
//! ride a side channel keyed by byte range (see [`super::core::SyntaxError`]).

use crate::syntax::SyntaxKind;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// Open a node of the given kind.
    Start(SyntaxKind),
    /// Attach the token at this index in the token stream.
    Tok(usize),
    /// Attach a `WORD` sub-token: the `start..end` byte slice of the token at
    /// `idx`. Math parsing uses these slices to recover TeX's one-token script
    /// boundaries from the lexer's coarser `WORD` runs (`a,b^2` and `x^2;`).
    /// Losslessness is preserved because the slices emitted for one token cover
    /// its full byte range contiguously (see [`super::grammar`]).
    SubTok {
        idx: usize,
        start: usize,
        end: usize,
    },
    /// Close the most recently opened node.
    Finish,
}

/// An open node that must be completed exactly once.
///
/// The marker owns no parser borrow, so nested grammar calls can keep emitting
/// events. Its start remains in place while those calls append children or wrap
/// them at later checkpoints. A forgotten completion trips the drop bomb in
/// debug builds, where the grammar call responsible is still on the stack.
#[must_use = "an opened parser node must be completed"]
pub(crate) struct Marker {
    pos: usize,
    bomb: DropBomb,
}

impl Marker {
    pub(crate) fn open(events: &mut Vec<Event>, kind: SyntaxKind) -> Self {
        let pos = events.len();
        events.push(Event::Start(kind));
        Self {
            pos,
            bomb: DropBomb { armed: true },
        }
    }

    /// Wrap events already emitted since `checkpoint`, when a node's kind only
    /// becomes known after its first children have been parsed.
    pub(crate) fn precede(events: &mut Vec<Event>, checkpoint: usize, kind: SyntaxKind) -> Self {
        events.insert(checkpoint, Event::Start(kind));
        Self {
            pos: checkpoint,
            bomb: DropBomb { armed: true },
        }
    }

    pub(crate) fn complete(mut self, events: &mut Vec<Event>) {
        debug_assert!(matches!(events.get(self.pos), Some(Event::Start(_))));
        events.push(Event::Finish);
        self.bomb.armed = false;
    }
}

struct DropBomb {
    armed: bool,
}

impl Drop for DropBomb {
    fn drop(&mut self) {
        // Preserve the original panic if parsing already failed. A second panic
        // in a destructor would abort the process before it can report the cause.
        if cfg!(debug_assertions) && self.armed && !std::thread::panicking() {
            panic!("parser marker dropped without completing its node");
        }
    }
}

/// Pull a completed construct's start back over its bound documentation comment.
/// Its existing finish still closes it, so this creates no new marker obligation.
pub(crate) fn extend_back(events: &mut Vec<Event>, checkpoint: usize, at: usize) {
    debug_assert!(checkpoint <= at, "extend_back must move a Start backwards");
    if let Event::Start(kind) = events[at] {
        events.remove(at);
        events.insert(checkpoint, Event::Start(kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_markers_preserve_event_order() {
        let mut events = Vec::new();
        let outer = Marker::open(&mut events, SyntaxKind::COMMAND);
        events.push(Event::Tok(0));
        let inner = Marker::open(&mut events, SyntaxKind::GROUP);
        events.push(Event::Tok(1));
        inner.complete(&mut events);
        outer.complete(&mut events);
        assert!(matches!(
            events.as_slice(),
            [
                Event::Start(SyntaxKind::COMMAND),
                Event::Tok(0),
                Event::Start(SyntaxKind::GROUP),
                Event::Tok(1),
                Event::Finish,
                Event::Finish,
            ]
        ));
    }

    #[test]
    fn precede_wraps_a_completed_node_and_its_script() {
        let mut events = Vec::new();
        let math = Marker::open(&mut events, SyntaxKind::MATH);
        let checkpoint = events.len();
        let base = Marker::open(&mut events, SyntaxKind::COMMAND);
        events.push(Event::Tok(0));
        base.complete(&mut events);
        let scripted = Marker::precede(&mut events, checkpoint, SyntaxKind::SCRIPTED);
        let script = Marker::open(&mut events, SyntaxKind::SUPERSCRIPT);
        events.push(Event::Tok(1));
        script.complete(&mut events);
        scripted.complete(&mut events);
        math.complete(&mut events);
        assert!(matches!(
            events.as_slice(),
            [
                Event::Start(SyntaxKind::MATH),
                Event::Start(SyntaxKind::SCRIPTED),
                Event::Start(SyntaxKind::COMMAND),
                Event::Tok(0),
                Event::Finish,
                Event::Start(SyntaxKind::SUPERSCRIPT),
                Event::Tok(1),
                Event::Finish,
                Event::Finish,
                Event::Finish,
            ]
        ));
    }

    #[test]
    fn extend_back_binds_comments_inside_the_completed_construct() {
        let mut events = Vec::new();
        let paragraph = Marker::open(&mut events, SyntaxKind::PARAGRAPH);
        let checkpoint = events.len();
        let comment = Marker::open(&mut events, SyntaxKind::DOC_COMMENT);
        events.push(Event::Tok(0));
        comment.complete(&mut events);
        let construct_start = events.len();
        let construct = Marker::open(&mut events, SyntaxKind::COMMAND);
        events.push(Event::Tok(1));
        construct.complete(&mut events);
        extend_back(&mut events, checkpoint, construct_start);
        paragraph.complete(&mut events);
        assert!(matches!(
            events.as_slice(),
            [
                Event::Start(SyntaxKind::PARAGRAPH),
                Event::Start(SyntaxKind::COMMAND),
                Event::Start(SyntaxKind::DOC_COMMENT),
                Event::Tok(0),
                Event::Finish,
                Event::Tok(1),
                Event::Finish,
                Event::Finish,
            ]
        ));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "parser marker dropped without completing its node")]
    fn dropping_an_open_marker_trips_the_bomb() {
        let _marker = Marker::open(&mut Vec::new(), SyntaxKind::COMMAND);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "parser marker dropped without completing its node")]
    fn dropping_a_preceding_marker_trips_the_bomb() {
        let mut events = vec![Event::Tok(0)];
        let _marker = Marker::precede(&mut events, 0, SyntaxKind::PARAGRAPH);
    }

    #[test]
    fn a_marker_does_not_panic_again_during_unwinding() {
        let panic = std::panic::catch_unwind(|| {
            let _marker = Marker::open(&mut Vec::new(), SyntaxKind::COMMAND);
            panic!("original parser panic");
        })
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original parser panic"));
    }
}
