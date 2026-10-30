import { mount } from "svelte";
import App from "./App.svelte";
import { loadLangPref } from "./lib/state.svelte";
import { toastUnhandledRejections } from "./lib/unhandled";
import "./app.css";

toastUnhandledRejections();

// 表記の言語（ui_lang）を読んでから描く（mount の後に切り替えると最初の 1 フレームが日本語で出る）
void loadLangPref().finally(() => mount(App, { target: document.getElementById("app")! }));
