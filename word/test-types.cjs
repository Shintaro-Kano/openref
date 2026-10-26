const fs=require("fs"),path=require("path"),vm=require("vm");
const dir=__dirname;
vm.runInThisContext(fs.readFileSync(path.join(dir,"vendor/citeproc.js"),"utf8"));
vm.runInThisContext(fs.readFileSync(path.join(dir,"cite-core.js"),"utf8"));
const locale=fs.readFileSync(path.join(dir,"vendor/locales-en-US.xml"),"utf8");
const style=fs.readFileSync(path.join(dir,"styles/vancouver.csl"),"utf8");
const cases={
  book:{type:"book",title:"画像診断コンパクトナビ",author:[{literal:"山田太郎"}],publisher:"医学書院","publisher-place":"東京",issued:{"date-parts":[[2025]]},edition:"3",ISBN:"978-4-260"},
  chapter:{type:"chapter",title:"膵腫瘍の画像診断","container-title":"腹部画像診断学",author:[{family:"Tanaka",given:"H"}],editor:[{family:"Sato",given:"K"}],publisher:"South",issued:{"date-parts":[[2024]]},page:"120-145"},
  webpage:{type:"webpage",title:"PI-RADS v2.1","container-title":"ACR",URL:"https://acr.org/pirads",issued:{"date-parts":[[2019]]},accessed:{"date-parts":[[2026,7,25]]}},
  report:{type:"report",title:"Imaging Guidelines 2026",author:[{literal:"日本医学放射線学会"}],publisher:"JRS",issued:{"date-parts":[[2026]]},number:"GL-3",genre:"Guideline"},
  thesis:{type:"thesis",title:"Deep learning for CT",author:[{family:"Kano",given:"S"}],publisher:"Univ of Tokyo",issued:{"date-parts":[[2020]]},genre:"PhD thesis"},
  "paper-conference":{type:"paper-conference",title:"AI in radiology","container-title":"RSNA Proceedings","event-title":"RSNA 2026","event-place":"Chicago",author:[{family:"Smith",given:"J"}],issued:{"date-parts":[[2026]]}},
  "article-newspaper":{type:"article-newspaper",title:"新しい画像診断技術","container-title":"日本経済新聞",issued:{"date-parts":[[2026,3,1]]},section:"科学"},
  document:{type:"document",title:"院内マニュアル",publisher:"○○病院",issued:{"date-parts":[[2026]]},genre:"内部資料"},
};
let fail=0;
for(const [name,csl] of Object.entries(cases)){
  try{
    const core=new OpenRefCite.CiteCore(CSL,style,locale);
    core.setItems({K:csl}); core.register(["K"]);
    const bib=core.bibliographyHtml().replace(/<[^>]+>/g,"").replace(/\s+/g," ").trim();
    if(!bib || bib.length<10) throw new Error("空の出力");
    console.log(`OK   ${name.padEnd(18)} ${bib.slice(0,88)}`);
  }catch(e){ fail++; console.log(`FAIL ${name}: ${e.message}`); }
}
console.log(fail?`\n${fail} 型が FAILED`:"\n全 8 型 OK");
process.exit(fail?1:0);
