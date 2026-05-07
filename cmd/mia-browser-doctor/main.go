package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"time"

	"github.com/snth-ai/snth-companion/internal/browser"
)

func main() {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	v, err := browser.PWVersion(ctx)
	if err != nil {
		fmt.Fprintln(os.Stderr, "version err:", err)
		os.Exit(1)
	}
	b, _ := json.Marshal(v)
	fmt.Println("version:", string(b))

	url, err := browser.PWNavigate(ctx, "https://example.com")
	if err != nil {
		fmt.Fprintln(os.Stderr, "nav err:", err)
		os.Exit(1)
	}
	fmt.Println("navigated to:", url)

	snap, err := browser.PWSnapshot(ctx)
	if err != nil {
		fmt.Fprintln(os.Stderr, "snap err:", err)
		os.Exit(1)
	}
	fmt.Println("snapshot title:", snap.Title)
	fmt.Println("snapshot url:", snap.URL)
	fmt.Println("snapshot has", len(snap.Selectors), "interactive elements")
	fmt.Println("snapshot text head:")
	if len(snap.Text) > 600 {
		fmt.Println(snap.Text[:600])
	} else {
		fmt.Println(snap.Text)
	}
}
