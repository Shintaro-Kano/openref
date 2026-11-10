import { mount } from "svelte";
import App from "./App.svelte";
import { loadDetailPref, loadLangPref, loadListSortPref, loadViewerSidePref } from "./lib/state.svelte";
import { toastUnhandledRejections } from "./lib/unhandled";
import "./app.css";

toastUnhandledRejections();

// 表記の言語（ui_lang）・一覧の右ペインの開閉（ui_detail）・一覧の並び（ui_list_sort）・PDF ビューアの右バーの開閉（ui_viewer_side）を読んでから描く
// （mount の後に切り替えると最初の 1 フレームが日本語で出る・右ペイン / 右バーが出て消える・既定の並びで 1 度取る）
void Promise.all([loadLangPref(), loadDetailPref(), loadListSortPref(), loadViewerSidePref()]).finally(() => mount(App, { target: document.getElementById("app")! }));
