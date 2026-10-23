// Promise Plugin contract pinned to OpenCode 74f8d4860f49c563909e6b039b0cc8b454ebce72.
// A structural Plugin object avoids package installation or a separate JS runtime.
import { readFile } from "node:fs/promises"

export default {
  id: "cruise-native-tools",
  setup: async (ctx) => {
    const bridge = JSON.parse(await readFile(new URL("./bridge.json", import.meta.url), "utf8"))
    const allowed = new Set(bridge.allowed)
    const questions = new Set(["question", "ask", "ask_user_question"])
    const call = async (path, body, signal) => {
      const response = await fetch(`${bridge.url}/${path}`, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: bridge.token },
        body: JSON.stringify({ sessionID: bridge.sessionID, ...body }),
        signal,
        redirect: "error",
      })
      if (!response.ok) throw new Error(`Cruise tool bridge HTTP ${response.status}`)
      const result = await response.json()
      if (result.error) throw new Error(result.error)
      return result
    }
    await ctx.tool.transform((editor) => {
      for (const tool of bridge.tools) {
        editor.add({
          name: tool.name,
          description: tool.description,
          input: tool.parameters,
          options: { codemode: false, permission: tool.name },
          execute: async (input, context) => {
            if (context.sessionID !== bridge.sessionID) throw new Error("Cruise bridge session mismatch")
            const result = await call("call", { callID: context.id, tool: tool.name, input }, context.signal)
            return { content: result.content }
          },
        })
      }
    })
    await ctx.session.hook("context", (event) => {
      if (event.sessionID !== bridge.sessionID) return
      for (const name of Object.keys(event.tools)) {
        if (questions.has(name) || (bridge.restricted && !allowed.has(name))) delete event.tools[name]
      }
    })
    await ctx.tool.hook("execute.before", async (event) => {
      if (event.sessionID !== bridge.sessionID) return
      if (questions.has(event.tool) || (bridge.restricted && !allowed.has(event.tool))) {
        throw new Error("Tool is not allowed by Cruise permissions")
      }
      await call("effect", { callID: event.id, tool: event.tool }, AbortSignal.timeout(5000))
    })
    await ctx.session.hook("retry", (event) => {
      if (event.sessionID === bridge.sessionID) event.decision = { retry: false }
    })
    await call("ready", {}, AbortSignal.timeout(5000))
  },
}
