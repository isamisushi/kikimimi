// Installed by Kikimimi Desktop alongside the OpenCode configuration.
import { Plugin } from "@opencode/plugin"

export default Plugin.define({
  id: "kikimimi-title-routing",
  async setup(ctx) {
    await ctx.session.hook("http.request", async (event) => {
      const request = event.request
      if (event.kind !== "title" || request.method !== "POST") return
      const body = await request.clone().json()
      const headers = new Headers(request.headers)
      headers.delete("content-length")
      event.request = new Request(request, {
        headers,
        body: JSON.stringify({ ...body, model: "kikimimi-low" }),
      })
    }, { providerID: "kikimimi" })
  },
})
