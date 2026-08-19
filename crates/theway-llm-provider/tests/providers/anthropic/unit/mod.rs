    use super::*;

    fn mk_model() -> Model {
        Model {
            id: "claude-test".into(),
            name: "Claude Test".into(),
            api: Api::known(KnownApi::AnthropicMessages),
            provider: Provider::from("anthropic"),
            base_url: "https://api.anthropic.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn cache_control_applied_to_system_and_last_user() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: Some("sys".into()),
            messages: vec![
                Message::User(UserMessage {
                    role: UserRole::User,
                    content: UserContent::Text("first".into()),
                    timestamp: 0,
                }),
                Message::User(UserMessage {
                    role: UserRole::User,
                    content: UserContent::Text("last".into()),
                    timestamp: 0,
                }),
            ],
            tools: None,
        };
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();

        let sys = &body["system"];
        assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");

        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(
            msgs[0]["content"][0]["cache_control"].is_null(),
            "first user should not have cache_control"
        );
        assert_eq!(
            msgs[1]["content"][0]["cache_control"]["type"], "ephemeral",
            "last user should have cache_control"
        );
    }

    #[test]
    fn long_retention_adds_ttl() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: Some("sys".into()),
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Long),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn temperature_dropped_when_thinking_enabled() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let mut opts = StreamOptions {
            temperature: Some(0.7),
            ..Default::default()
        };
        opts.provider_extras.insert(
            "thinking".into(),
            json!({ "type": "enabled", "budget_tokens": 4096 }),
        );
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();
        assert!(
            body.get("temperature").is_none(),
            "temperature must be dropped"
        );
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[test]
    fn tools_get_cache_control_on_last() {
        let m = mk_model();
        let tools = vec![
            Tool {
                name: "a".into(),
                description: "a".into(),
                parameters: json!({ "type": "object" }),
            },
            Tool {
                name: "b".into(),
                description: "b".into(),
                parameters: json!({ "type": "object" }),
            },
        ];
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: Some(tools),
        };
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Short),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();
        let tools_v = body["tools"].as_array().unwrap();
        assert!(
            tools_v[0].get("cache_control").is_none(),
            "first tool should not have cc"
        );
        assert_eq!(tools_v[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn fireworks_compat_disables_cache_on_tools() {
        let mut m = mk_model();
        m.provider = Provider::from("fireworks");
        let compat = resolve_compat(&m);
        assert!(!compat.supports_cache_control_on_tools);
        assert!(!compat.supports_long_cache_retention);
        assert!(compat.send_session_affinity_headers);
    }
