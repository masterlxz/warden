//! The feed of activity (P121) for the screens: `ListActivity` reads what the hub already keeps (`warden_bootstrap::activity`). Read only, and
//! the owner's: it shows what agents asked each other and answered, which a member has no business reading in the owner's files, so a member
//! gets an empty list.

use std::path::Path;

use warden_bootstrap::activity::{read_activity, ActivityEvent, MAX_EVENTS};
use warden_server_protocol::protocol::ActivityEventDto;
use warden_server_protocol::ServerMessage;

/// An event as the screens get it. Also what the desktop shows for this computer.
pub fn to_dto(event: ActivityEvent) -> ActivityEventDto {
    ActivityEventDto {
        id: event.id,
        at_ms: event.at_ms,
        kind: event.kind.to_string(),
        actor: event.actor,
        target: event.target,
        text: event.text,
        task_id: event.task_id,
        conversation_id: event.conversation_id,
    }
}

/// Answers `ListActivity`. `tasks_log` is `None` on a hub that keeps none; `conversations_dirs` are the owner's (the device's and the
/// scheduled tasks').
pub fn handle_list_activity(tasks_log: Option<&Path>, conversations_dirs: &[&Path], is_owner: bool, request_id: u64) -> ServerMessage {
    let events = if is_owner { read_activity(tasks_log, conversations_dirs, MAX_EVENTS).into_iter().map(to_dto).collect() } else { Vec::new() };
    ServerMessage::ActivityList { request_id, events }
}
