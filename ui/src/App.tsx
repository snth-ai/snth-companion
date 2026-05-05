import { HashRouter, Route, Routes } from "react-router-dom"
import { Layout } from "@/components/Layout"
import { StatusPage } from "@/pages/Status"
import { PairPage } from "@/pages/Pair"
import { SynthsPage } from "@/pages/Synths"
import { ChannelsPage } from "@/pages/Channels"
import { KeysPage } from "@/pages/Keys"
import { CodexLoginPage } from "@/pages/CodexLogin"
import { ToolsPage } from "@/pages/Tools"
import { SynthToolsPage } from "@/pages/SynthTools"
import { PrivacyPage } from "@/pages/Privacy"
import { SandboxPage } from "@/pages/Sandbox"
import { LogsPage } from "@/pages/Logs"
import { AppsPage } from "@/pages/Apps"
import { KnowledgePage } from "@/pages/Knowledge"
import { RememberedPage } from "@/pages/Remembered"
import { DreamsPage } from "@/pages/Dreams"
import { LibraryPage } from "@/pages/Library"
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
            <Route path="synths" element={<SynthsPage />} />
            <Route path="channels" element={<ChannelsPage />} />
            <Route path="keys" element={<KeysPage />} />
            <Route path="login/codex" element={<CodexLoginPage />} />
            <Route path="tools" element={<ToolsPage />} />
            <Route path="synth-tools" element={<SynthToolsPage />} />
            <Route path="privacy" element={<PrivacyPage />} />
            <Route path="sandbox" element={<SandboxPage />} />
            <Route path="logs" element={<LogsPage />} />
            <Route path="apps" element={<AppsPage />} />
            <Route path="knowledge" element={<KnowledgePage />} />
            <Route path="remembered" element={<RememberedPage />} />
            <Route path="dreams" element={<DreamsPage />} />
            <Route path="library" element={<LibraryPage />} />
          </Route>
        </Routes>
      </HashRouter>
      <Toaster />
    </>
  )
}
