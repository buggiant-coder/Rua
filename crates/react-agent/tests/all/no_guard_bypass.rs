use std::sync::Arc;

use react_agent::hooks::NoopHook;
use react_agent::test_support::MockChatModel;
use react_agent::{ReactAgent, ReactConfig};

#[test]
fn builder_does_not_wrap_hook() {
    let noop: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let noop_ptr = Arc::as_ptr(&noop) as *const ();

    let model = Arc::new(MockChatModel::text_response("ok"));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .hook(noop.clone())
        .build();

    let ctx_hook_ptr = Arc::as_ptr(&agent.ctx.hook) as *const ();
    assert_eq!(
        noop_ptr, ctx_hook_ptr,
        "hook must be passed through without wrapping — GuardHook bypass removed"
    );
}
