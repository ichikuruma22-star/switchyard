use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // An optional scenario precedes agy's own arguments.
    let scenario = args.get(1).filter(|arg| !arg.starts_with("--")).map_or("complete", String::as_str);
    std::fs::write("received-args.txt", args[1..].join("\n")).unwrap();
    let resumed = args.windows(2).find(|pair| pair[0] == "--conversation").map(|pair| pair[1].clone());
    let conversation = match (&resumed, scenario) {
        (Some(_), "forked") => "00000000-0000-4000-8000-000000000001".to_owned(),
        (Some(saved), _) => saved.clone(),
        (None, _) => "123e4567-e89b-12d3-a456-426614174000".to_owned(),
    };
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
        if writeln!(stdout, "{line}").and_then(|()| stdout.flush()).is_err() {
            // The adapter stopped reading after a protocol fault.
            return;
        }
    }
}
