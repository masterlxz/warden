mod support;

use support::{spin_up_server, MockProvider};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};
use warden_server_protocol::protocol::SkillDto;

fn dto(name: &str) -> SkillDto {
    SkillDto { name: name.into(), description: "Reviews a PR".into(), body: "Step 1.".into(), agents: Vec::new() }
}

#[tokio::test]
async fn skills_can_be_created_listed_edited_and_deleted_over_the_socket() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "ext-1", "Extension", "test-key").await.unwrap();

    conn.send(&ClientMessage::SaveSkill { request_id: 1, skill: dto("review-pr"), overwrite: false }).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::SkillOk { request_id: 1 })));

    // A second create with the same name is refused, and the connection keeps working.
    conn.send(&ClientMessage::SaveSkill { request_id: 2, skill: dto("review-pr"), overwrite: false }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::SkillError { request_id: 2, message }) => assert!(message.contains("already exists")),
        other => panic!("expected SkillError, got {other:?}"),
    }

    let edited = SkillDto { body: "Step 1.\nStep 2.".into(), ..dto("review-pr") };
    conn.send(&ClientMessage::SaveSkill { request_id: 3, skill: edited.clone(), overwrite: true }).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::SkillOk { request_id: 3 })));

    conn.send(&ClientMessage::ListSkills { request_id: 4 }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::SkillList { request_id: 4, skills }) => assert_eq!(skills, vec![edited]),
        other => panic!("expected SkillList, got {other:?}"),
    }

    conn.send(&ClientMessage::DeleteSkill { request_id: 5, name: "review-pr".into() }).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::SkillOk { request_id: 5 })));

    conn.send(&ClientMessage::ListSkills { request_id: 6 }).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::SkillList { request_id: 6, skills }) if skills.is_empty()));
}
