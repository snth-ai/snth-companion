import { HashRouter, Route, Routes } from "react-router-dom"
import { Layout } from "@/components/Layout"
import { StatusPage } from "@/pages/Status"
import { PairPage } from "@/pages/Pair"
import { ChannelsPage } from "@/pages/Channels"
import { KeysPage } from "@/pages/Keys"
import { CodexLoginPage } from "@/pages/CodexLogin"
import { ToolsPage } from "@/pages/Tools"
import { SandboxPage } from "@/pages/Sandbox"
import { LogsPage } from "@/pages/Logs"
import { Toaster } from "@/components/ui/sonner"

// HashRouter (not BrowserRouter) so in-page navigation never triggers
// a server request — the Go server only serves /ui/index.html +
// /ui/assets/* and legacy /*, /pair, /channels… pages.

export default function App() {
  return (
    <>
      <HashRouter>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<StatusPage />} />
            <Route path="pair" element={<PairPage />} />
            <Route path="channels" element={<ChannelsPage />} />
            <Route path="keys" element={<KeysPage />} />
            <Route path="login/codex" element={<CodexLoginPage />} />
            <Route path="tools" element={<ToolsPage />} />
            <Route path="sandbox" element={<SandboxPage />} />
            <Route path="logs" element={<LogsPage />} />
          </Route>
        </Routes>
      </HashRouter>
      <Toaster />
    </>
  )
}
