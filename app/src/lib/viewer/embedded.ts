/** Read-only PDF markup. No links, forms, actions, attachments or rich HTML. */
const markup = new Set(["Text","FreeText","Line","Square","Circle","Polygon","PolyLine","Highlight","Underline","Squiggly","StrikeOut","Stamp","Caret","Ink"]);
export interface EmbeddedComment {id:string;page:number;kind:string;author:string;text:string}
export function embeddedComments(annotations:any[], page:number):EmbeddedComment[] {
  const found=new Map<string,EmbeddedComment>();
  for(const a of annotations) {
    if((a.hidden || (a.annotationFlags & 2)) || !markup.has(a.subtype))continue;
    const text=typeof a.contentsObj?.str==="string"?a.contentsObj.str:typeof a.contents==="string"?a.contents:"";
    const author=typeof a.titleObj?.str==="string"?a.titleObj.str:"";
    found.set(String(a.id),{id:String(a.id),page,kind:String(a.subtype),author,text});
  }
  // A Popup may carry the parent's comment. Keep one card per markup object.
  for(const a of annotations) {
    if(a.subtype!=="Popup" || a.hidden)continue;
    const parent=found.get(String(a.parentId));
    if(parent && !parent.text && typeof a.contentsObj?.str==="string")parent.text=a.contentsObj.str;
  }
  return [...found.values()];
}
export function inertMarkup(annotations:any[]):any[] {
  return annotations.filter(a=>!a.hidden && !(a.annotationFlags & 2) && !(a.subtype==="Text" && a.hasAppearance) && markup.has(a.subtype)).map(a=>({
    ...a,contentsObj:{str:"",dir:"ltr"},titleObj:{str:"",dir:"ltr"},contents:"",richText:null,popupRef:null,
    actions:null,url:null,unsafeUrl:null,dest:null,action:null,attachment:null,
  }));
}
