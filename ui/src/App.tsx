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
import { MemoryPage } from "@/pages/Memory"
import { DreamsPage } from "@/pages/Dreams"
import { LibraryPage } from "@/pages/Library"
import { GraphPage } from "@/pages/Graph"
import { DiagnosticsPage } from "@/pages/Diagnostics"
import { ContextPage } from "@/pages/Context"
import { TasksPage } from "@/pages/Tasks"
import { TaskTemplatesPage } from "@/pages/TaskTemplates"
import { SynthSettingsPage } from "@/pages/SynthSettings"
import { PublicPage } from "@/pages/Public"
import { PlacesPage } from "@/pages/Places"
import { MCPPage } from "@/pages/MCP"
import { SkillsPage } from "@/pages/Skills"
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
            <Route path="memory" element={<MemoryPage />} />
            <Route path="dreams" element={<DreamsPage />} />
            <Route path="library" element={<LibraryPage />} />
            <Route path="graph" element={<GraphPage />} />
            <Route path="diagnostics" element={<DiagnosticsPage />} />
            <Route path="context" element={<ContextPage />} />
            <Route path="tasks" element={<TasksPage />} />
            <Route path="task-templates" element={<TaskTemplatesPage />} />
            <Route path="synth-settings" element={<SynthSettingsPage />} />
            <Route path="public" element={<PublicPage />} />
            <Route path="places" element={<PlacesPage />} />
            <Route path="mcp" element={<MCPPage />} />
            <Route path="skills" element={<SkillsPage />} />
          </Route>
        </Routes>
      </HashRouter>
      <Toaster />
    </>
  )
}
