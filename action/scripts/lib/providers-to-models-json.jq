# Validates the `providers` action input and emits the pi `models.json`
# document it describes. Replaces the old ~5-chained-`jq -e`-calls-per-provider
# loop in setup-env.sh: that approach can only ever check one flat rule at a
# time and can't build nested model/compat objects, so the full schema (model
# objects, cost, compat, headers, no_auth) moved here instead.
#
# Invocation contract (see action/scripts/setup-env.sh):
#   stdin           the parsed `providers` JSON object
#   --argjson keyrefs   {providerId: "CRUISE_PROVIDER_API_KEY_N", ...} for every
#                    keyed (non-`no_auth`) provider; a `no_auth: true` provider
#                    is, by construction, absent from this map.
#   --argjson reserved  lowercase snapshot of pi's built-in provider ids/aliases
#                    (see reserved_warning below).
#   stdout (exactly one of):
#     {"error": "<message>"}                       on the first validation
#                                                    failure (no `::error::`
#                                                    prefix -- bash adds that)
#     {"models_json": {...}, "warnings": [...]}     on success
#
# All validation is expressed as `error(<string>)` calls caught by the single
# top-level `try/catch` at the bottom, so bash never has to parse jq's own
# stderr formatting -- it gets one clean, always-JSON answer either way.
#
# Provider ids are iterated via `keys`, which jq always returns sorted, so
# the first error encountered (and the emitted `providers` object) are both
# deterministic run to run.

# Generic whitespace trim, used both for "non-blank" checks below and for
# the reserved-id comparison (pi's provider_metadata() does `.trim()` then
# an ASCII-case-insensitive compare -- provider_metadata.rs:1659-1672).
def trimmed: sub("^[ \t\r\n]+"; "") | sub("[ \t\r\n]+$"; "");

# ---------------------------------------------------------------------------
# Rule 1: provider id shape.
# ---------------------------------------------------------------------------
def id_regex: "^[A-Za-z0-9][A-Za-z0-9._-]*$";

# Rule 5/6: model id shape -- deliberately more permissive than a provider id
# (any non-whitespace run), matching what pi itself accepts as a model id.
def model_id_regex: "^[^[:space:]]+\\z";

# ---------------------------------------------------------------------------
# Rule 2: provider object key allowlist.
# ---------------------------------------------------------------------------
def required_provider_keys: ["api", "base_url", "models"];
def optional_provider_keys: ["headers", "auth_header", "compat", "no_auth"];
def allowed_provider_keys: required_provider_keys + optional_provider_keys;

# Rule 6: model object key allowlist ("id" is the only required one).
def allowed_model_keys:
  ["id", "name", "api", "reasoning", "input", "cost", "context_window", "max_tokens", "headers", "compat"];

# Rule 8: the 16 known `compat` keys, split by how they validate/emit.
def compat_bool_keys:
  ["supports_store", "supports_developer_role", "supports_reasoning_effort",
   "supports_usage_in_streaming", "supports_tools", "supports_streaming",
   "supports_parallel_tool_calls", "force_adaptive_thinking"];
def compat_string_keys: ["max_tokens_field", "system_role_name", "stop_reason_field", "thinking_format"];
# custom_headers / thinking_level_map are handled by name below (each needs
# its own validator); open_router_routing / vercel_gateway_routing fall
# through to the opaque-object branch.
def allowed_compat_keys:
  compat_bool_keys + compat_string_keys +
  ["custom_headers", "thinking_level_map", "open_router_routing", "vercel_gateway_routing"];

# snake_case -> camelCase is always spelled out explicitly, NEVER a generic
# gsub: `headers`/`custom_headers`/`thinking_level_map` map *keys*, and
# `open_router_routing`/`vercel_gateway_routing` *values*, are opaque
# pass-through data (arbitrary header names, thinking-level names, raw JSON
# merged into the request body) that must survive byte-for-byte -- a
# programmatic snake->camel transform would have no way to know not to touch
# them, since it only sees strings, not schema position.
#
# ProviderConfig, ModelConfig and ModelCost each rename only two fields, at a
# single emission site apiece, so those are written inline where they are
# emitted. `compat` gets the table below because its 16 fields are emitted by
# a loop over the user's keys, which needs the lookup.
def compat_camel_map: {
  supports_store: "supportsStore",
  supports_developer_role: "supportsDeveloperRole",
  supports_reasoning_effort: "supportsReasoningEffort",
  supports_usage_in_streaming: "supportsUsageInStreaming",
  supports_tools: "supportsTools",
  supports_streaming: "supportsStreaming",
  supports_parallel_tool_calls: "supportsParallelToolCalls",
  max_tokens_field: "maxTokensField",
  system_role_name: "systemRoleName",
  stop_reason_field: "stopReasonField",
  custom_headers: "customHeaders",
  open_router_routing: "openRouterRouting",
  vercel_gateway_routing: "vercelGatewayRouting",
  thinking_level_map: "thinkingLevelMap",
  force_adaptive_thinking: "forceAdaptiveThinking",
  thinking_format: "thinkingFormat"
};

# ---------------------------------------------------------------------------
# Rule 3: the `api` allowlist, shared verbatim by provider-level and
# model-level `api` (rule 6 says "same allowlist as rule 3").
# ---------------------------------------------------------------------------
def allowed_apis:
  ["anthropic-messages", "openai-completions", "openai-responses", "google-generative-ai",
   "azure-openai-responses", "bedrock-converse-stream", "cohere-chat"];
# These three need credentials pi cannot mint from a static string: a
# ChatGPT OAuth JWT (chatgpt_account_id claim), a JSON {token, projectId}
# from a GCP OAuth flow, and a ~1h GCP access token respectively -- see
# result_adapters.md. Calling them out by name (instead of the generic
# "unsupported api" error) points the user at the escape hatch that still
# works for them.
def excluded_apis: ["openai-codex-responses", "google-gemini-cli", "google-vertex"];

def validate_api($label; $val):
  if ($val | type) != "string" then
    error("\($label) api must be a string")
  elif (allowed_apis | index($val)) then
    $val
  elif (excluded_apis | index($val)) then
    error("\($label) has api '\($val)', which pi can only drive with an OAuth JWT or a GCP-minted access token -- a static per-provider API key cannot satisfy it. Use 'pi_models_json' (which can now be combined with 'providers') to configure this adapter instead.")
  else
    error("\($label) has unsupported api '\($val)'")
  end;

# ---------------------------------------------------------------------------
# Rule 4: base_url.
# ---------------------------------------------------------------------------
def validate_base_url($id; $val):
  # The `\z` anchor (not `$`) is deliberate: `$` in Oniguruma also matches
  # just before a trailing newline, which would let a base_url ending in
  # "\n" slip through.
  if ($val | type) == "string" and ($val | test("^https?://[^[:space:]]+\\z")) then
    $val
  else
    error("provider '\($id)' has invalid base_url (expected non-empty http:// or https:// URL)")
  end;

# ---------------------------------------------------------------------------
# Rule 7: header maps (provider-level `headers`, model-level `headers`, and
# `compat.custom_headers` all share this validator).
# ---------------------------------------------------------------------------
def header_name_ok($k): ($k | type) == "string" and ($k | length) > 0 and (($k | test("\\s")) | not) and (($k | test("[[:cntrl:]]")) | not) and (($k | contains(":")) | not);
# Two rejections here, for two different reasons:
#  - `!`-prefixed values: resolve_value_with_base (models.rs:2185) runs a
#    `!`-prefixed value through `sh -c`, so a generated config must never
#    become a shell-exec vector. `env:`/`file:` stay allowed.
#  - control characters (CR/LF included): reqwest refuses to build a header
#    from them, so pi fails deep in the request path with a message that
#    points nowhere near the workflow input that caused it. Catching it here
#    is the whole point of validating instead of passing through.
def header_value_ok($v): ($v | type) == "string" and ($v | length) > 0 and (($v | startswith("!")) | not) and (($v | test("[[:cntrl:]]")) | not);

def validate_headers($label; $val):
  if ($val | type) != "object" then
    error("\($label) must be an object")
  elif ($val | length) == 0 then
    error("\($label) must be a non-empty object")
  else
    ($val | to_entries | map(select(header_name_ok(.key) | not))) as $bad_names
    | ($val | to_entries | map(select(header_name_ok(.key) and (header_value_ok(.value) | not)))) as $bad_values
    | if ($bad_names | length) > 0 then
        error("\($label) has an invalid header name '\($bad_names[0].key)' (must be non-empty, with no whitespace, control characters or ':')")
      elif ($bad_values | length) > 0 then
        error("\($label) has an invalid value for header '\($bad_values[0].key)' (must be a non-empty string with no control characters, and must not start with '!')")
      else
        $val
      end
  end;

# thinking_level_map values are internal pi strings (thinking-level names),
# never routed through resolve_value_with_base, so they get the lighter
# non-empty-string-map check instead of the header-specific rules above.
def validate_string_map($label; $val):
  if ($val | type) != "object" then
    error("\($label) must be an object")
  elif ($val | length) == 0 then
    error("\($label) must be a non-empty object")
  else
    ($val | to_entries | map(select((.value | type) != "string" or (.value | length) == 0))) as $bad
    | if ($bad | length) > 0 then
        error("\($label) has an invalid (empty or non-string) value for key '\($bad[0].key)'")
      else
        $val
      end
  end;

# ---------------------------------------------------------------------------
# Rule 8: compat.
# ---------------------------------------------------------------------------
def validate_compat($label; $val):
  if ($val | type) != "object" then
    error("\($label) must be an object")
  elif ($val | length) == 0 then
    error("\($label) must be a non-empty object")
  else
    (($val | keys) - allowed_compat_keys) as $unknown
    | if ($unknown | length) > 0 then
        error("\($label) has unknown key '\($unknown[0])' (allowed keys: \(allowed_compat_keys | sort | join(", ")))")
      else
        reduce ($val | keys)[] as $k (
          {};
          . + (
            (compat_camel_map[$k]) as $camel
            | if (compat_bool_keys | index($k)) then
                if ($val[$k] | type) == "boolean" then {($camel): $val[$k]}
                else error("\($label).\($k) must be a boolean") end
              elif (compat_string_keys | index($k)) then
                if ($val[$k] | type) == "string" and ($val[$k] | length) > 0 then {($camel): $val[$k]}
                else error("\($label).\($k) must be a non-empty string") end
              elif $k == "custom_headers" then
                {($camel): validate_headers("\($label).custom_headers"; $val[$k])}
              elif $k == "thinking_level_map" then
                {($camel): validate_string_map("\($label).thinking_level_map"; $val[$k])}
              else
                # open_router_routing / vercel_gateway_routing: opaque
                # pass-through merged verbatim into the request body by pi --
                # contents intentionally not validated.
                if ($val[$k] | type) == "object" then {($camel): $val[$k]}
                else error("\($label).\($k) must be an object") end
              end
          )
        )
      end
  end;

# ---------------------------------------------------------------------------
# Model-level scalar fields.
# ---------------------------------------------------------------------------
def validate_model_input($id; $mid; $val):
  if ($val | type) != "array" or ($val | length) == 0 then
    error("provider '\($id)' model '\($mid)' input must be a non-empty array of unique values, each 'text' or 'image'")
  else
    ($val | map(select(. != "text" and . != "image"))) as $bad
    | if ($bad | length) > 0 then
        # pi itself doesn't error here -- it silently drops any value other
        # than "text"/"image" (models.rs:2008-2025) -- so without this check
        # a typo would go unnoticed instead of just being ignored.
        error("provider '\($id)' model '\($mid)' input contains an unsupported value '\($bad[0])' (pi accepts only \"text\"/\"image\" and silently drops anything else)")
      elif ($val | length) != ($val | unique | length) then
        error("provider '\($id)' model '\($mid)' input must not contain duplicate values")
      else
        $val
      end
  end;

# ModelCost (provider.rs:202-210) derives neither Default nor
# #[serde(default)], so a partial `cost` object fails to deserialize at all
# and takes the *whole* models.json down with it -- hence "exactly these
# four, all required" rather than "these four are optional".
def validate_model_cost($id; $mid; $val):
  if ($val | type) != "object" then
    error("provider '\($id)' model '\($mid)' cost must be an object")
  elif (($val | keys | sort) != ["cache_read", "cache_write", "input", "output"]) then
    error("provider '\($id)' model '\($mid)' cost must contain exactly 'input', 'output', 'cache_read', and 'cache_write' (pi's ModelCost has no defaults, so a partial cost object fails the whole models.json parse)")
  else
    (["input", "output", "cache_read", "cache_write"] | map(select(($val[.] | type) != "number" or $val[.] < 0))) as $bad
    | if ($bad | length) > 0 then
        error("provider '\($id)' model '\($mid)' cost.\($bad[0]) must be a number >= 0 (per-million-tokens)")
      else
        {input: $val.input, output: $val.output, cacheRead: $val.cache_read, cacheWrite: $val.cache_write}
      end
  end;

# pi reads context_window/max_tokens as u32.
def validate_u32($id; $mid; $field; $val):
  if ($val | type) != "number" or $val != ($val | floor) or $val <= 0 or $val > 4294967295 then
    error("provider '\($id)' model '\($mid)' \($field) must be an integer > 0 and <= 4294967295 (pi reads it as a u32)")
  else
    $val
  end;

# ---------------------------------------------------------------------------
# Rule 6: a `models[]` element given as an object.
# ---------------------------------------------------------------------------
def validate_model_object($id; $m):
  (($m | keys) - allowed_model_keys) as $unknown
  | if ($unknown | length) > 0 then
      error("provider '\($id)' model has unknown key '\($unknown[0])' (allowed optional keys: name, api, reasoning, input, cost, context_window, max_tokens, headers, compat)")
    elif ($m | has("id") | not) then
      error("provider '\($id)' model is missing required key 'id'")
    elif ($m.id | type) != "string" or ($m.id | test(model_id_regex) | not) then
      error("provider '\($id)' model has an invalid id '\($m.id | tostring)'")
    else
      ($m.id) as $mid
      | ({id: $mid}
         + (if ($m | has("name")) then
              if ($m.name | type) == "string" and ($m.name | length) > 0 then {name: $m.name}
              else error("provider '\($id)' model '\($mid)' has an invalid name") end
            else {} end)
         + (if ($m | has("api")) then {api: validate_api("provider '\($id)' model '\($mid)'"; $m.api)} else {} end)
         + (if ($m | has("reasoning")) then
              if ($m.reasoning | type) == "boolean" then {reasoning: $m.reasoning}
              else error("provider '\($id)' model '\($mid)' has a non-boolean 'reasoning'") end
            else {} end)
         + (if ($m | has("input")) then {input: validate_model_input($id; $mid; $m.input)} else {} end)
         + (if ($m | has("cost")) then {cost: validate_model_cost($id; $mid; $m.cost)} else {} end)
         + (if ($m | has("context_window")) then {contextWindow: validate_u32($id; $mid; "context_window"; $m.context_window)} else {} end)
         + (if ($m | has("max_tokens")) then {maxTokens: validate_u32($id; $mid; "max_tokens"; $m.max_tokens)} else {} end)
         + (if ($m | has("headers")) then {headers: validate_headers("provider '\($id)' model '\($mid)' headers"; $m.headers)} else {} end)
         + (if ($m | has("compat")) then {compat: validate_compat("provider '\($id)' model '\($mid)' compat"; $m.compat)} else {} end))
    end;

# ---------------------------------------------------------------------------
# Rules 5/6: the `models` array as a whole (string or object elements,
# resolved ids unique across the array).
# ---------------------------------------------------------------------------
def validate_models($id; $val):
  if ($val | type) != "array" or ($val | length) == 0 then
    error("provider '\($id)' models must be a non-empty array of unique, non-empty model ids")
  else
    ($val | map(
       if type == "string" then
         if test(model_id_regex) then {resolved_id: ., emitted: {id: .}}
         else error("provider '\($id)' models must be a non-empty array of unique, non-empty model ids") end
       elif type == "object" then
         validate_model_object($id; .) as $built | {resolved_id: $built.id, emitted: $built}
       else
         error("provider '\($id)' models must be a non-empty array of unique, non-empty model ids")
       end
     )) as $entries
    | ($entries | map(.resolved_id)) as $ids
    | if ($ids | length) != ($ids | unique | length) then
        error("provider '\($id)' models must be a non-empty array of unique, non-empty model ids")
      else
        $entries | map(.emitted)
      end
  end;

# ---------------------------------------------------------------------------
# Rule 2: provider object shape (required keys present, no unrecognized
# keys). pi's own structs use no #[serde(deny_unknown_fields)] anywhere
# (verified: zero occurrences), so a misspelled key would otherwise be
# silently ignored instead of failing loudly here.
# ---------------------------------------------------------------------------
def validate_provider_shape($id; $val):
  if ($val | type) != "object" then
    error("provider '\($id)' must be a JSON object")
  else
    (required_provider_keys - ($val | keys)) as $missing
    | if ($missing | length) > 0 then
        error("provider '\($id)' is missing required key '\($missing[0])'")
      else
        (($val | keys) - allowed_provider_keys) as $unknown
        | if ($unknown | length) > 0 then
            error("provider '\($id)' has unknown key '\($unknown[0])' (allowed optional keys: headers, auth_header, compat, no_auth)")
          else
            $val
          end
      end
  end;

# ---------------------------------------------------------------------------
# no_auth header injection (see result_auth.md / p2_keyless.md): none of the
# six allowed adapters consult ProviderConfig.authHeader when deciding
# whether a key is required -- that field only feeds a separate,
# CLI-startup-level readiness check. The only mechanism that actually
# suppresses every adapter's "Missing API key" error is a non-empty
# case-insensitive match on one of these header names, checked *before* the
# adapter resolves options.api_key at all.
# ---------------------------------------------------------------------------
# The recognized names differ per adapter, so this MUST be scoped by `api`:
# openai-completions / openai-responses / cohere-chat check `authorization`
# only, anthropic-messages also accepts `x-api-key`, google-generative-ai
# `x-goog-api-key`, azure-openai-responses `api-key`. Treating them as one
# flat union would accept e.g. cohere-chat + `api-key`, skip the injection,
# and hand pi a config that hard-errors at request time -- the exact outcome
# this validation exists to prevent.
def override_header_names($api):
  ["authorization"]
  + (if $api == "anthropic-messages" then ["x-api-key"]
     elif $api == "google-generative-ai" then ["x-goog-api-key"]
     elif $api == "azure-openai-responses" then ["api-key"]
     else [] end);

# A per-model `api` can select a different adapter than the provider-level
# one, so a user-supplied override header only counts when EVERY adapter that
# could dispatch for this provider recognizes it -- i.e. the intersection.
# `authorization` is in all six sets, so the intersection is never empty.
def recognized_override_names($apis):
  ($apis | unique | map(override_header_names(.))) as $sets
  # `// []` guards the empty-$apis seed: the reduce body never runs, so the
  # seed would surface as `null` and make every name look unrecognized. Not
  # reachable today (validate_api guarantees a provider-level api), but the
  # safe direction is an empty set -- unrecognized means "inject the
  # placeholder", which fails closed.
  | reduce $sets[] as $s (($sets[0] // []); . - (. - $s));

def has_nonblank_override($merged; $names):
  ($merged | to_entries | map(select(
      (.key | ascii_downcase) as $k
      | ($names | index($k))
        and ((.value | type) == "string")
        and ((.value | trimmed | length) > 0)
    )) | length) > 0;

# ---------------------------------------------------------------------------
# Assemble one provider entry, in the exact rule order given (each stops the
# whole program at the first failure via `error()`, caught once at the very
# bottom).
# ---------------------------------------------------------------------------
def build_provider($id; $raw):
  (if ($id | test(id_regex)) then true else error("invalid provider id '\($id)' in 'providers'") end) as $_id_ok
  | validate_provider_shape($id; $raw) as $val
  | validate_api("provider '\($id)'"; $val.api) as $api
  | validate_base_url($id; $val.base_url) as $base_url
  | validate_models($id; $val.models) as $models
  | (if ($val | has("headers")) then validate_headers("provider '\($id)' headers"; $val.headers) else null end) as $prov_headers
  | (if ($val | has("compat")) then validate_compat("provider '\($id)' compat"; $val.compat) else null end) as $prov_compat
  | (if ($val | has("auth_header")) then
       (if ($val.auth_header | type) == "boolean" then $val.auth_header
        else error("provider '\($id)' auth_header must be a boolean") end)
     else null end) as $auth_header
  | (if ($val | has("no_auth")) then
       (if ($val.no_auth | type) == "boolean" then $val.no_auth
        else error("provider '\($id)' no_auth must be a boolean") end)
     else false end) as $no_auth
  # Every api that could dispatch for this provider: its own, plus any
  # per-model override. Both the Bedrock rejection and the override-header
  # check below have to consider all of them, not just the provider-level one.
  | (([$api] + ($models | map(.api // empty))) | unique) as $effective_apis
  | (if $no_auth and ($effective_apis | index("bedrock-converse-stream")) then
       error("provider '\($id)' cannot set no_auth with api 'bedrock-converse-stream' (AWS auth is never absent, and pi's Bedrock auth-override path is unverified)")
     else null end)
  # A colliding id makes pi ignore the declared `api` entirely (dispatch
  # matches the provider id first, providers/mod.rs:182-240), so there is no
  # way to know here which adapter runs, and therefore no way to guarantee the
  # injected header suppresses its missing-key error. Advisory warning is not
  # enough when correctness depends on the answer.
  | (if $no_auth and ($reserved | index($id | trimmed | ascii_downcase)) then
       error("provider '\($id)' cannot set no_auth because its id collides with a pi built-in provider: request dispatch matches the provider id before the 'api' field (providers/mod.rs:182-240), so which adapter runs -- and therefore which header suppresses its missing-key error -- cannot be determined here. Rename the provider to a non-colliding id.")
     else null end)
  | ({api: $api, baseUrl: $base_url}
     + (if $prov_headers != null then {headers: $prov_headers} else {} end)
     + (if $auth_header != null then {authHeader: $auth_header} else {} end)
     + (if $prov_compat != null then {compat: $prov_compat} else {} end)
     + {models: $models}) as $emitted
  | if ($keyrefs | has($id)) then
      $emitted + {apiKey: ("env:" + $keyrefs[$id])}
    else
      (($emitted.headers // {}) + ($emitted.compat.customHeaders // {})) as $combined_headers
      | if has_nonblank_override($combined_headers; recognized_override_names($effective_apis)) then
          $emitted
        else
          # Non-empty is load-bearing: apply_headers_ignoring_blank_auth_overrides
          # (providers/mod.rs:61-75) skips sending a header whose value is
          # blank, and resolve_value_with_base drops empty literals before
          # they even reach that map -- an empty placeholder would silently
          # fail to suppress the missing-key error it's meant to prevent.
          $emitted + {headers: (($emitted.headers // {}) + {authorization: "no-auth"})}
        end
    end;

# ---------------------------------------------------------------------------
# Rule (warnings): built-in provider id collisions.
# ---------------------------------------------------------------------------
def reserved_warning($id):
  ($id | trimmed | ascii_downcase) as $norm
  | if ($reserved | index($norm)) then
      ["provider '\($id)' collides with a pi built-in provider id -- pi will resolve its credential from that built-in provider's own source first (so the configured 'provider_api_keys' value may never be used), and will silently ignore this provider's 'api' field, because request dispatch matches the provider id before the api string (providers/mod.rs:182-240)"]
    else
      []
    end;

# ---------------------------------------------------------------------------
# Top level: build every provider (sorted, deterministic), collect warnings,
# and turn any error() raised anywhere above into the `{"error": ...}` shape
# instead of jq's own stderr formatting.
# ---------------------------------------------------------------------------
try (
  . as $providers
  | reduce ($providers | keys)[] as $id (
      {providers: {}, warnings: []};
      ($providers[$id]) as $raw
      | .providers[$id] = build_provider($id; $raw)
      | .warnings += reserved_warning($id)
    )
  | {models_json: {providers: .providers}, warnings: .warnings}
) catch {error: .}
