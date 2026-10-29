import assert from "node:assert/strict";
import {embeddedComments,inertMarkup} from "./embedded.ts";
const annotations=[
 {id:"h",subtype:"Highlight",contentsObj:{str:"Comment <script>inert</script>"},titleObj:{str:"Author"},actions:{JS:"do not run"},url:"https://example.org",richText:{str:"<b>HTML</b>"}},
 {id:"t",subtype:"Text",contentsObj:{str:""}},
 {id:"p",subtype:"Popup",parentId:"t",contentsObj:{str:"Popup text"}},
 {id:"l",subtype:"Link",url:"https://example.org"},
 {id:"w",subtype:"Widget",fieldType:"Tx"},
 {id:"f",subtype:"FileAttachment",file:{filename:"a"}},
 {id:"hidden",subtype:"Highlight",hidden:true},
];
assert.deepEqual(embeddedComments(annotations,3).map(a=>[a.id,a.page,a.text]),[["h",3,"Comment <script>inert</script>"],["t",3,"Popup text"]]);
const safe=inertMarkup(annotations);
assert.equal(safe.length,2);assert.equal(safe[0].actions,null);assert.equal(safe[0].richText,null);assert.equal(safe[0].url,null);assert.equal(safe[0].contentsObj.str,"");
assert.equal(annotations[0].contentsObj.str,"Comment <script>inert</script>");
console.log("Embedded PDF annotations: comments, popup deduplication, inert markup and source preservation passed");
const appearance=[{id:"ap",subtype:"Text",hasAppearance:true,name:"NoIcon",contentsObj:{str:"AP comment"}},{id:"plain",subtype:"Text",hasAppearance:false},{id:"flagHidden",subtype:"Text",annotationFlags:2}];
assert.deepEqual(inertMarkup(appearance).map(a=>a.id),["plain"]);
assert.deepEqual(embeddedComments(appearance,1).map(a=>a.id),["ap","plain"]);
console.log("AP Text uses canvas appearance without a duplicate icon; Hidden flag excluded");
