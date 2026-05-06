import { NavLink, Outlet } from "react-router-dom"
import {
  Activity,
  Book,
  Brain,
  Cloud,
  Image,
  KeyRound,
  Network,
  LayoutGrid,
  Link as LinkIcon,
  Lock,
  LogIn,
  MessageSquare,
  ScrollText,
  Settings,
  Shield,
  Users,
  Wrench,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { SynthSwitcher } from "@/components/SynthSwitcher"

const navItems = [
  { to: "/", label: "Status", icon: Activity },
  { to: "/apps", label: "Apps", icon: LayoutGrid },
  { to: "/knowledge", label: "Knowledge", icon: Book },
  { to: "/graph", label: "Graph", icon: Network },
  { to: "/remembered", label: "Remembered", icon: Brain },
  { to: "/dreams", label: "Dreams", icon: Cloud },
  { to: "/library", label: "Library", icon: Image },
  { to: "/pair", label: "Pair", icon: LinkIcon },
  { to: "/synths", label: "Synths", icon: Users },
  { to: "/channels", label: "Channels", icon: MessageSquare },
  { to: "/keys", label: "API Keys", icon: KeyRound },
  { to: "/login/codex", label: "Codex Login", icon: LogIn },
  { to: "/tools", label: "Tools", icon: Wrench },
  { to: "/synth-tools", label: "Synth Tools", icon: Settings },
  { to: "/privacy", label: "Privacy", icon: Lock },
  { to: "/sandbox", label: "Sandbox", icon: Shield },
  { to: "/logs", label: "Logs", icon: ScrollText },
]

export function Layout() {
  return (
    <div className="dark min-h-svh bg-background text-foreground">
      <div className="flex min-h-svh">
        <aside className="w-60 shrink-0 border-r border-border bg-card/40">
          <div className="px-5 pt-6 pb-4">
            <div className="text-lg font-semibold tracking-tight">
              SNTH Companion
            </div>
            <div className="text-xs text-muted-foreground mt-1">
              local sidecar
            </div>
          </div>
          <div className="px-3 pb-3 border-b border-border">
            <SynthSwitcher />
          </div>
          <nav className="px-2 py-2 space-y-0.5">
            {navItems.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.to === "/"}
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-2 rounded-md px-3 py-2 text-sm",
                    "transition-colors",
                    isActive
                      ? "bg-primary/10 text-foreground"
                      : "text-muted-foreground hover:bg-muted hover:text-foreground",
                  )
                }
              >
                <item.icon className="h-4 w-4" />
                <span>{item.label}</span>
              </NavLink>
            ))}
          </nav>
        </aside>
        <main className="flex-1 min-w-0">
          <div className="max-w-5xl mx-auto px-8 py-10">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  )
}
