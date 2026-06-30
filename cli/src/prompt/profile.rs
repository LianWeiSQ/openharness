use super::*;

const BUILD_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/build.txt");
const EXPLORE_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/explore.txt");
const PLAN_AGENT_PROMPT: &str = include_str!("../../../skill/prompts/plan.txt");

include!("profile/types.rs");
include!("profile/model.rs");
include!("profile/loading.rs");
include!("profile/subagents.rs");
include!("profile/parse.rs");
include!("profile/builtin.rs");
include!("profile/binding.rs");
include!("profile/permissions.rs");
