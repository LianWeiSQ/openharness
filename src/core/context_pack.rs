#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextItem {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub content: String,
    pub priority: i64,
    pub token_estimate: u64,
    pub pinned: bool,
    pub stable_prefix: bool,
    pub ttl_turns: Option<u64>,
    pub metadata: BTreeMap<String, Value>,
}

impl ContextItem {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        kind: impl Into<String>,
        source: impl Into<String>,
        content: impl Into<String>,
        priority: i64,
    ) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            source: source.into(),
            content: content.into(),
            priority,
            token_estimate: 0,
            pinned: false,
            stable_prefix: false,
            ttl_turns: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextPackTraceEntry {
    pub item_id: String,
    pub kind: String,
    pub source: String,
    pub priority: i64,
    pub pinned: bool,
    pub stable_prefix: bool,
    pub token_estimate: u64,
    pub included: bool,
    pub drop_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextPack {
    pub messages: Vec<ChatMessage>,
    pub items: Vec<ContextItem>,
    pub trace: Vec<ContextPackTraceEntry>,
    pub estimated_input_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ContextPackBuildOptions {
    pub token_budget: Option<u64>,
    pub bytes_per_token: u64,
    pub trace_only: bool,
}

impl Default for ContextPackBuildOptions {
    fn default() -> Self {
        Self {
            token_budget: None,
            bytes_per_token: DEFAULT_BYTES_PER_TOKEN,
            trace_only: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ContextPackBuilder {
    pub options: ContextPackBuildOptions,
}

impl ContextPackBuilder {
    #[must_use]
    pub fn new(options: Option<ContextPackBuildOptions>) -> Self {
        Self {
            options: options.unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn build(&self, input: ContextPackInput) -> ContextPack {
        let mut items = self.collect_items(&input);
        items = self.with_estimates(self.dedupe_items(items));
        let trace = self.project(&items);
        let included_ids = trace
            .iter()
            .filter(|entry| entry.included)
            .map(|entry| entry.item_id.clone())
            .collect::<BTreeSet<_>>();
        let estimated_input_tokens = items
            .iter()
            .filter(|item| included_ids.contains(&item.id))
            .map(|item| item.token_estimate)
            .sum();
        let messages = if self.options.trace_only {
            input.messages
        } else {
            items
                .iter()
                .filter(|item| included_ids.contains(&item.id))
                .map(item_to_message)
                .collect()
        };
        ContextPack {
            messages,
            items,
            trace,
            estimated_input_tokens,
        }
    }

    #[must_use]
    pub fn collect_items(&self, input: &ContextPackInput) -> Vec<ContextItem> {
        let mut items = Vec::new();
        if let Some(runtime_context) = input.runtime_context.as_ref().map(|item| item.trim())
            && !runtime_context.is_empty()
        {
            let mut item =
                ContextItem::new("runtime:current", "runtime", "runtime", runtime_context, 90);
            item.pinned = true;
            item.metadata
                .insert("synthetic".to_string(), Value::Bool(true));
            items.push(item);
        }
        if let Some(work_state) = work_state_item(&input.metadata, input.messages.len()) {
            items.push(work_state);
        }
        let empty_execution = Value::Object(Map::new());
        let execution = input
            .sandbox_metadata
            .as_ref()
            .or_else(|| input.metadata.get("execution"))
            .unwrap_or(&empty_execution);
        if let Some(sandbox) = sandbox_item(execution) {
            items.push(sandbox);
        }
        if let Some(todo) = todo_item(&input.todos) {
            items.push(todo);
        }
        items.extend(message_items(&input.messages));
        items.extend(input.extra_items.clone());
        items
    }

    fn with_estimates(&self, items: Vec<ContextItem>) -> Vec<ContextItem> {
        items
            .into_iter()
            .map(|mut item| {
                if item.token_estimate == 0 {
                    item.token_estimate =
                        estimate_text_tokens(&item.content, self.options.bytes_per_token);
                }
                item
            })
            .collect()
    }

    fn project(&self, items: &[ContextItem]) -> Vec<ContextPackTraceEntry> {
        let mut included = BTreeSet::new();
        let mut dropped = BTreeMap::new();
        let mut used = 0u64;
        let mut ranked = items.iter().enumerate().collect::<Vec<_>>();
        ranked.sort_by(|(left_index, left), (right_index, right)| {
            (
                !left.pinned,
                -left.priority,
                i64::try_from(*left_index).unwrap_or(i64::MAX),
            )
                .cmp(&(
                    !right.pinned,
                    -right.priority,
                    i64::try_from(*right_index).unwrap_or(i64::MAX),
                ))
        });
        for (_index, item) in ranked {
            if self.options.token_budget.is_none_or(|budget| budget == 0)
                || item.pinned
                || used + item.token_estimate <= self.options.token_budget.unwrap_or(0)
            {
                included.insert(item.id.clone());
                used += item.token_estimate;
            } else {
                dropped.insert(item.id.clone(), "budget".to_string());
            }
        }
        items
            .iter()
            .map(|item| {
                let is_included = included.contains(&item.id);
                ContextPackTraceEntry {
                    item_id: item.id.clone(),
                    kind: item.kind.clone(),
                    source: item.source.clone(),
                    priority: item.priority,
                    pinned: item.pinned,
                    stable_prefix: item.stable_prefix,
                    token_estimate: item.token_estimate,
                    included: is_included,
                    drop_reason: if is_included {
                        None
                    } else {
                        Some(
                            dropped
                                .get(&item.id)
                                .cloned()
                                .unwrap_or_else(|| "not_selected".to_string()),
                        )
                    },
                }
            })
            .collect()
    }

    fn dedupe_items(&self, items: Vec<ContextItem>) -> Vec<ContextItem> {
        let mut by_id = BTreeMap::<String, ContextItem>::new();
        let mut order = Vec::new();
        for item in items {
            if let Some(existing) = by_id.get(&item.id) {
                if item_rank(&item) > item_rank(existing) {
                    by_id.insert(item.id.clone(), item);
                }
            } else {
                order.push(item.id.clone());
                by_id.insert(item.id.clone(), item);
            }
        }
        order
            .into_iter()
            .filter_map(|item_id| by_id.remove(&item_id))
            .collect()
    }
}
