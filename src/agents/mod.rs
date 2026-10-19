//! The AI coding agents platoon runs, behind one seam. Each agent has a module of its own here (`claude.rs`: Claude
//! Code) that is the only place that knows its files, flags, hooks and models; the rest of platoon goes through this
//! one.

pub mod claude;
