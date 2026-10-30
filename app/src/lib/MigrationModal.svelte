<script lang="ts">
  import { onMount } from "svelte";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { migrationApi, type MigrationConfig, type MigrationStatus, type MigrationHistory } from "./api";
  import { app, refreshAll, reveal } from "./state.svelte";
  import { t } from "./i18n.svelte";

  let config = $state<MigrationConfig>({format:"paperpile",path:"",roots:[],mode:"copy",source_id:null,adopt_legacy_ids:false});
  let job = $state<MigrationStatus|null>(null);
  let history = $state<MigrationHistory[]>([]);
  let error = $state("");
  let pending = $state(false);
  let itemChoices = $state<Record<string,string>>({});
  let fileChoices = $state<Record<string,string>>({});
  let choicesDirty = $state(false);
  let loadedPlan = "";
  const fileId = (item:string,att:string)=>JSON.stringify([item,att]);
  let cancelling = $state(false);
  let page = $state(0);
  const active = $derived(job?.phase === "preparing" || job?.phase === "running" || job?.phase === "indexing");
  const frozen = $derived(active || job?.phase === "ready" || pending);
  const plan = $derived(job?.plan);
  const report = $derived(job?.report);
  const entries = $derived(plan?.items ?? report?.results ?? []);
  const priorSources = $derived([...new Map([...history].reverse().filter(h=>h.format===config.format).map(h=>[h.source_id,h])).values()]);
  let pollBusy = false;
  let pollAgain = false;
  let alive = true;
  let dismissedId = "";

  async function poll() {
    if (pollBusy) { pollAgain=true;return; }
    pollBusy = true;
    try {
      const next = await migrationApi.status();
      if (!alive) return;
      if (next?.id === dismissedId) return;
      const finished = active && !["running","preparing","indexing"].includes(next?.phase ?? "");
      job = next;
      if (next?.phase === "ready" && next.plan && next.id !== loadedPlan) {
        loadedPlan=next.id;
        if (next.config) config=next.config;
        itemChoices=Object.fromEntries(next.config?.decisions?.items.map(c=>[c.object_id,c.item_key]) ?? []);
        fileChoices=Object.fromEntries(next.config?.decisions?.files.map(c=>[fileId(c.object_id,c.attachment_id),c.path]) ?? []);
        choicesDirty=false;
      }
      if (next?.plan && !config.source_id) config.source_id=next.plan.source_id;
      if (finished) { cancelling = false; await refreshAll(); history = await migrationApi.history(); }
    } catch (e) { if (alive) error = String(e); }
    finally { pollBusy = false;if(pollAgain && alive){pollAgain=false;void poll();} }
  }
  onMount(() => {
    void poll();
    void migrationApi.history().then(h=>history=h).catch(e=>error=String(e));
    const timer = setInterval(()=>{if(active || pending) void poll();},600);
    return ()=>{alive=false;clearInterval(timer);};
  });
  async function chooseFile() {
    try {
      const p=await open({multiple:false,filters:[{name:config.format==="paperpile"?"Paperpile JSON":"Zotero RDF",extensions:config.format==="paperpile"?["json"]:["rdf","xml"]}]});
      if(typeof p==="string") config.path=p;
    } catch(e){error=String(e);}
  }
  async function chooseRoots() {
    try {
      const p=await open({directory:true,multiple:true});
      const paths=Array.isArray(p)?p:typeof p==="string"?[p]:[];
      config.roots=[...new Set([...config.roots,...paths])];
    } catch(e){error=String(e);}
  }
  async function preview() {
    pending=true;error="";page=0;cancelling=false;
    try {await migrationApi.preview($state.snapshot(config));dismissedId="";await poll();}
    catch(e){error=String(e);}finally{pending=false;}
  }
  async function confirmChoices() {
    if(!plan)return;
    config.decisions={source_id:plan.source_id,input_sha256:plan.input_sha256,
      items:Object.entries(itemChoices).filter(([,key])=>key).map(([object_id,item_key])=>({object_id,item_key,csl_sha256:plan.items.find(i=>i.object_id===object_id)?.candidate_details.find(c=>c.key===item_key)?.csl_sha256 ?? config.decisions?.items.find(c=>c.object_id===object_id && c.item_key===item_key)?.csl_sha256})),
      files:Object.entries(fileChoices).filter(([,path])=>path).map(([key,path])=>{const [object_id,attachment_id]=JSON.parse(key);return {object_id,attachment_id,path};})};
    await preview();
  }
  async function clearChoices() {config.decisions=undefined;itemChoices={};fileChoices={};choicesDirty=false;await preview();}
  async function start() {
    if(!job || pending || choicesDirty)return;
    pending=true;error="";page=0;
    try{await migrationApi.start(job.id);await poll();}catch(e){error=String(e);}finally{pending=false;}
  }
  async function cancel() {
    if(!job)return;
    cancelling=true;
    try{await migrationApi.cancel(job.id);await poll();}catch(e){error=String(e);cancelling=false;}
  }
  async function saveReport() {
    if(!job)return;
    try{const path=await save({defaultPath:`openref-import-${report?.run_id ?? job.id}.json`,filters:[{name:"JSON",extensions:["json"]}]});if(path)await migrationApi.save(job.id,path);}
    catch(e){error=String(e);}
  }
  async function loadReport(h:MigrationHistory) {
    pending=true;error="";
    try {
      await migrationApi.selectReport(h.id);dismissedId="";page=0;
      config={format:h.format==="zotero"?"zotero":"paperpile",path:"",roots:[],mode:h.attachment_mode==="link"?"link":"copy",source_id:h.source_id,adopt_legacy_ids:false};
      await poll();
    } catch(e){error=String(e);}finally{pending=false;}
  }
  async function index() {
    if(!job)return;pending=true;error="";
    try{await migrationApi.index(job.id);await poll();}catch(e){error=String(e);}finally{pending=false;}
  }
  function reset() {dismissedId=job?.id ?? "";job=null;error="";page=0;config.decisions=undefined;itemChoices={};fileChoices={};choicesDirty=false;loadedPlan="";}
  const labels:Record<string,string>=$derived({new:t("新規"),add_missing:t("既存へ追加"),review:t("要確認"),blocked:t("保留"),inserted:t("登録"),merged:t("既存へ追加"),failed:t("失敗"),available:t("利用可能"),existing:t("保存済み"),missing:t("不足"),ambiguous:t("曖昧"),unsupported:t("対象外"),copied:t("コピー済み"),linked:t("リンク済み"),attached:t("添付済み"),cancelled:t("中止"),finished:t("処理終了"),preparing:t("事前確認中…"),ready:t("事前確認"),running:t("移行中…"),error:t("失敗"),indexing:t("全文索引を作成中…"),retry:t("再試行が必要"),unresolved:t("未解決"),not_applied:t("未適用"),relocated:t("保存先を再設定"),pending:t("待機"),interrupted:t("中断")});
  const phases:Record<string,string>=$derived({database:t("ライブラリを確認中…"),preview:t("事前確認中…"),verify:t("PDF を確認中…"),copy:t("PDF をコピー中…"),item:t("文献を処理中…"),committed:t("登録済み"),fulltext:t("全文索引を作成中…")});
</script>

<svelte:window onkeydown={(e)=>{if(app.modal==="migration" && e.key==="Escape"){e.stopPropagation();app.modal=null;}}}/>
{#if app.modal === "migration"}
  <div class="overlay" role="presentation">
    <div class="modal mg-modal" role="dialog" aria-modal="true" aria-label={t("他の文献管理ソフトから移行")} tabindex="-1">
      <div class="modal-head"><h3>{t("他の文献管理ソフトから移行")}</h3><button class="x" onclick={()=>app.modal=null} aria-label={t("閉じる")}>×</button></div>
      <div class="modal-body mg-body">
        <details open={!plan&&!report}><summary>{t("入力と保存方法")}</summary><fieldset disabled={frozen}>
          <label>{t("移行元")} <select bind:value={config.format} onchange={()=>{config.path="";config.roots=[];config.source_id=null;}}><option value="paperpile">Paperpile</option><option value="zotero">Zotero</option></select></label>
          <p class="mg-help">{config.format==="paperpile"?t("Paperpile からライブラリ全体の JSON と PDF の ZIP を書き出し、ZIP を Finder で展開してください。JSON と展開フォルダを選択します。PDF 注釈を残すには書出し時に注釈を含めます。"):t("Zotero の Export Library で Zotero RDF を選び、ノート・ファイル・PDF 注釈を含めて書き出してください。RDF を選択すると、同じフォルダ内の files も読み取ります。登録・更新日時は引き継げません。")}</p>
          <button onclick={chooseFile}>{t("書誌ファイルを選択…")}</button><p class="mg-path">{config.path || t("未選択")}</p>
          <button onclick={chooseRoots}>{t("PDF フォルダを追加…")}</button>
          {#each config.roots as root}<div class="mg-attroot"><span class="mg-path">{root}</span><button aria-label={t("除外")} onclick={()=>config.roots=config.roots.filter(p=>p!==root)}>×</button></div>{/each}
          <label>{t("PDF の保存方法")} <select bind:value={config.mode}><option value="copy">{t("この Mac の OpenRef 保存領域へコピー（推奨）")}</option><option value="link">{t("元ファイルへのリンク")}</option></select></label>
          {#if config.mode==="link"}<p class="mg-help">{t("参照先を移動すると PDF を開けなくなります。ZIP の展開フォルダも削除せず保持してください。")}</p>{/if}
          <label>{t("以前の移行に追加")} <select bind:value={config.source_id}><option value={null}>{t("移行元を自動選択（複数の場合は選択必須）")}</option><option value="@new">{t("新しい移行元")}</option>{#if config.source_id && !priorSources.some(h=>h.source_id===config.source_id)}<option value={config.source_id}>{t("今回の移行元")}</option>{/if}{#each priorSources as h}<option value={h.source_id}>{h.date_added} · {h.total} · {h.source_id.slice(0,24)}</option>{/each}</select></label>
          <p class="mg-help">{t("同じライブラリの再実行・不足 PDF の補完では以前の移行を選びます。別のライブラリは新しい移行元として扱ってください。")}</p>
          {#if config.format==="paperpile"}<label class="mg-help"><input type="checkbox" bind:checked={config.adopt_legacy_ids}/>{t("以前に旧 Paperpile 取込を使った場合：既存の Paperpile ID は今回と同じライブラリのものです")}</label>{/if}
        </fieldset></details>
        {#if history.length && !frozen}<details><summary>{t("過去の移行結果")}</summary>{#each history as h}<p><button onclick={()=>loadReport(h)}>{h.date_added} · {h.format} · {h.total} · {labels[h.state] ?? h.state}</button></p>{/each}</details>{/if}
        {#if error || job?.error}<p class="mg-error" role="alert">{error || job?.error}</p>{/if}
        {#if job}<h4>{labels[job.phase] ?? job.phase}</h4>{/if}
        {#if job?.progress && active}<p role="status">{phases[job.progress.phase] ?? t("処理中…")} · {job.progress.completed} / {job.progress.total} · {(job.progress.bytes/1048576).toFixed(1)} MiB</p>{/if}
        {#if plan}
          <div class="mg-counts">
            <span>{t("新規")}: {plan.counts.new_items}</span><span>{t("既存へ追加")}: {plan.counts.existing_items}</span><span>{t("要確認")}: {plan.counts.review}</span><span>{t("保留")}: {plan.counts.blocked}</span>
            <span>PDF: {plan.counts.available_pdfs}</span><span>{t("不足")}: {plan.counts.missing_pdfs}</span><span>{t("曖昧")}: {plan.counts.ambiguous_pdfs}</span><span>{t("対象外")}: {plan.counts.unsupported_attachments}</span><span>{t("失敗")}: {plan.counts.failed_pdfs}</span>
            <span>{t("フォルダ")}: {plan.counts.folders}</span><span>{t("タグ所属")}: {plan.counts.tags}</span><span>{t("メモ")}: {plan.counts.notes}</span>
          </div>
          {#if config.mode==="copy"}<p class="mg-path">{t("コピー先")}: {plan.managed_storage}<br/>{t("コピー予定容量（上限）")}: {(plan.counts.copy_bytes_upper_bound/1048576).toFixed(2)} MiB</p>{/if}
          {#if plan.counts.review+plan.counts.blocked>0}<p class="mg-help">{t("確認候補の文献・PDFを選び、選択内容で再確認してください。未選択の候補と削除済み・識別子矛盾の文献は保留します。")}</p>{/if}
          {#each plan.warnings as w}<p class="mg-help">{w}</p>{/each}
        {/if}
        {#if report}
          <p>{labels[report.state] ?? report.state} · {report.results.length} / {report.total} · {report.complete?t("移行完了"):t("処理結果と不足・注意事項を確認してください")}</p>
          {#if report.fulltext_pending}<p class="mg-help">{t("全文索引の未処理")}: {report.fulltext_pending}</p><button disabled={active||pending} onclick={index}>{t("全文索引を作成・再試行")}</button>{/if}
          {#if report.backup_path}<p class="mg-path">{t("開始前バックアップ")}: {report.backup_path}</p>{/if}
          {#each report.warnings as w}<p class="mg-help">{w}</p>{/each}
        {/if}
        {#each entries.slice(page*50,(page+1)*50) as entry}
          <details class="mg-result"><summary>{"title" in entry && entry.title ? String(entry.title) : entry.object_id} · {"action" in entry ? labels[entry.action.action] ?? entry.action.action : labels[entry.status] ?? entry.status}</summary>
            {#if "action" in entry && entry.action.reason}<p>{entry.action.reason}</p>{/if}
            {#if "action" in entry}{#each entry.candidate_details as candidate}<button onclick={()=>{app.modal=null;void reveal(candidate.key);}}>{t("候補を開く")}: {candidate.title || candidate.key}</button>{/each}{/if}
            {#if "action" in entry && (entry.candidate_details.length || entry.allow_new) && job?.phase==="ready"}
              <label>{t("同じ文献として追加する候補")}
                <select disabled={pending} value={itemChoices[entry.object_id] ?? ""} onchange={e=>{itemChoices[entry.object_id]=e.currentTarget.value;choicesDirty=true;}}>
                  <option value="">{t("選ばず保留")}</option>
                  {#if entry.allow_new}<option value="@new">{t("新規として登録")}</option>{/if}
                  {#each entry.candidate_details as c}<option value={c.key}>{c.title} · {c.doi ?? c.pmid ?? c.key} · {c.key}</option>{/each}
                </select>
              </label>
            {/if}
            {#if "item_key" in entry && entry.item_key}<button onclick={()=>{app.modal=null;void reveal(entry.item_key!);}}>{t("文献を開く")}</button>{/if}
            {#each entry.warnings as w}<p class="mg-help">{w}</p>{/each}
            {#if "action" in entry && job?.phase==="ready"}
              {#each entry.attachments as att}
                {@const options=entry.file_options.filter(o=>o.attachment_id===att.id)}
                {#if options.length}
                  <label>{t("使用するPDF")}: {att.id}
                    <select disabled={pending} value={fileChoices[fileId(entry.object_id,att.id)] ?? ""} onchange={e=>{fileChoices[fileId(entry.object_id,att.id)]=e.currentTarget.value;choicesDirty=true;}}>
                      <option value="">{t("選ばず保留")}</option>
                      {#each options as o}<option value={o.path}>{o.path}</option>{/each}
                    </select>
                  </label>
                {/if}
              {/each}
            {/if}
            {#each entry.attachments as att}<p class="mg-path">{att.id} · {labels[att.status] ?? att.status}{#if "path" in att}<br/>{att.path}{/if}{#if "reason" in att} · {att.reason}{/if}{#if "detail" in att} · {att.detail}{/if}{#if "candidates" in att}{#each att.candidates ?? [] as p}<br/>{p}{/each}{/if}</p>{/each}
          </details>
        {/each}
        {#if entries.length>50}<div class="mg-pages"><button disabled={page===0} onclick={()=>page--}>{t("前へ")}</button><span>{page+1} / {Math.ceil(entries.length/50)}</span><button disabled={(page+1)*50>=entries.length} onclick={()=>page++}>{t("次へ")}</button></div>{/if}
      </div>
      <div class="modal-foot">
        {#if active}<button disabled={cancelling} onclick={cancel}>{cancelling?t("中止します…"):t("中止")}</button><button onclick={()=>app.modal=null}>{t("バックグラウンドで続行")}</button>
        {:else if job?.phase==="ready"}<button onclick={reset}>{t("入力を変更")}</button>{#if choicesDirty}<button disabled={pending} onclick={confirmChoices}>{t("選択内容で再確認")}</button>{/if}<button class="primary" disabled={pending||choicesDirty} onclick={start}>{t("移行を開始")}</button>
        {:else}{#if config.decisions}<button disabled={pending} onclick={clearChoices}>{t("選択を取り消して再確認")}</button>{/if}<button disabled={!config.path||pending} class="primary" onclick={preview}>{report?t("同じ入力で再確認・不足分を補完"):t("事前確認")}</button>{#if report}<button onclick={saveReport}>{t("結果 JSON を保存…")}</button><button onclick={reset}>{t("別の入力を選択")}</button>{/if}<button onclick={()=>app.modal=null}>{t("閉じる")}</button>{/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .mg-modal{width:min(850px,94vw);max-height:90vh;display:flex;flex-direction:column}.mg-body{overflow:auto;min-height:0}.mg-modal fieldset{border:0;padding:0;margin:0;display:grid;gap:10px}.mg-modal fieldset>button{justify-self:start}.mg-body button{border:1px solid var(--border);border-radius:6px;padding:5px 12px;background:var(--bg)}.mg-body button:hover:not(:disabled){border-color:var(--accent)}.mg-body button:disabled{opacity:.5;cursor:default}.mg-attroot button{padding:0 8px}.mg-body>button{align-self:flex-start}.mg-result button{margin:2px 6px 2px 0}.mg-modal label{display:flex;gap:12px;align-items:center}.mg-modal select{max-width:65%;padding:5px}.mg-path{overflow-wrap:anywhere;font-size:12px}.mg-help{font-size:12px;line-height:1.55;color:var(--dim);margin:5px 0}.mg-attroot{display:flex;justify-content:space-between;gap:10px}.mg-counts{display:flex;gap:10px;flex-wrap:wrap;margin:12px 0}.mg-counts span{padding:4px 7px;background:var(--accent-soft);border-radius:4px}.mg-result{border-top:1px solid var(--border);padding:8px 0;font-size:12px;overflow-wrap:anywhere}.mg-result summary{cursor:pointer}.mg-error{color:var(--danger);white-space:pre-wrap}.mg-pages{display:flex;gap:12px;align-items:center;margin-top:12px}
</style>
