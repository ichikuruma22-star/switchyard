use std::io::Write;

fn main() {
    let conversation = "123e4567-e89b-12d3-a456-426614174000";
    let lines = [
        format!(
            r#"{{"event":"init","conversation_id":"{conversation}","init":{{"cwd":"fixture","tools":["view_file"]}}}}"#
        ),
        format!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{conversation}","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"Fixture "}}}}"#
        ),
        format!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{conversation}","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"answer"}}}}"#
        ),
        format!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{conversation}","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":""}}}}"#
        ),
        format!(
            r#"{{"event":"step_update","step_update":{{"conversation_id":"{conversation}","step_index":2,"state":"DONE","step_type":"tool","tool_name":"view_file","tool_info":{{"name":"view_file","parameters":{{"AbsolutePath":"/fixture/alpha.txt"}},"output":"2 lines, 12 bytes"}}}}}}"#
        ),
        format!(
            r#"{{"event":"result","result":{{"conversation_id":"{conversation}","status":"SUCCESS","response":"Fixture answer","num_turns":1}}}}"#
        ),
    ];
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}").unwrap();
        stdout.flush().unwrap();
    }
}
