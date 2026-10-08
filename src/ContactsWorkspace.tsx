import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type TextareaHTMLAttributes } from "react";
import { CircleAlert, Check, ContactRound, Heart, LoaderCircle, Plus, Search, Sparkles, Trash, UserRound, Users } from "lucide-react";
import { ContactManagement, MergeContactsDialog } from "./ContactManagement";
import { ContactAddressField, ContactLinksField } from "./ContactAddressField";
import { mailClient } from "./data/client";
import type { ContactFieldSuggestion, ContactGroup, ContactProfile, ContactTimelineItem, SaveContactRequest } from "./domain";
import { readAiFeatures, readAiRequestConfig } from "./aiSettings";
import { errorMessage } from "./errors";
import { PanelResizeHandle, useContactListWidth } from "./PanelResizeHandle";
import { KeepInTouchSection } from "./KeepInTouchSection";
import { ContactGroupDetail, ContactGroupList, ContactGroupPicker, ContactGroupsSection } from "./ContactGroupViews";
import { filterGroups, groupsForContact } from "./contactGroups";
import { ContactFilesSection, ContextSectionHeader, RecentEmailsSection } from "./ContextSections";
import { KEEP_IN_TOUCH_FREQUENCIES, KEEP_IN_TOUCH_GROUPS, UPCOMING_BIRTHDAY_DAYS, describeBirthday, describeDue, formatKeepInTouchDate, frequencyLabel, isKeepInTouchDue, keepInTouchStatus, lastTouchAt, nextBirthday } from "./keepInTouch";

import type { ContactsView } from "./contactsView";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

export type { ContactsView };
const byGroupName = (a:ContactGroup,b:ContactGroup) => a.name.localeCompare(b.name,undefined,{sensitivity:"base"})||a.id.localeCompare(b.id);
const matchesQuery = (profile:ContactProfile,needle:string) => !needle || `${profile.displayName??""} ${profile.addresses.join(" ")} ${profile.company??""}`.toLocaleLowerCase().includes(needle);

const empty = (): SaveContactRequest => ({ id:null,displayName:"",role:"",company:"",location:"",bio:"",notes:"",links:[],photoData:null,favorite:false,addresses:[],birthday:"" });
const draftFrom = (item:ContactProfile): SaveContactRequest => ({ id:item.id,displayName:item.displayName??"",role:item.role??"",company:item.company??"",location:item.location??"",bio:item.bio??"",notes:item.notes??"",links:item.links,photoData:item.photoData,favorite:item.favorite,addresses:item.addresses,birthday:item.birthday??"" });
// Everything Save contact writes. Favorite saves on its own, so it never
// leaves the form with unsaved changes.
const formSnapshot = (value:SaveContactRequest) => JSON.stringify([value.displayName??"",value.role??"",value.company??"",value.location??"",value.bio??"",value.notes??"",value.links,value.photoData??null,value.addresses,value.birthday??""]);
// Optional fields stay collapsed to an "Add" button until they hold a value
// or the user asks for them, so empty inputs don't crowd out real details.
type OptionalField = "company"|"location"|"birthday"|"links"|"bio"|"notes";
const OPTIONAL_FIELDS:{field:OptionalField;label:string}[] = [{field:"company",label:"Company"},{field:"location",label:"Location"},{field:"birthday",label:"Birthday"},{field:"links",label:"Links"},{field:"bio",label:"About"},{field:"notes",label:"Notes"}];
const optionalFieldHasValue = (value:Pick<SaveContactRequest,OptionalField>,field:OptionalField) => field==="links"?value.links.length>0:!!(value[field]??"").trim();
const CONTACT_SUGGESTION_FIELD_LABELS:Record<ContactFieldSuggestion["field"],string> = { displayName:"Name",role:"Role",company:"Company",location:"Location",bio:"About",link:"Link" };
// The excerpt is the exact source text that justifies the suggestion. For a
// value copied verbatim from the email (a link, most often) it's identical to
// the suggested value, so showing it again underneath is pure noise.
const suggestionHasDistinctEvidence = (item:ContactFieldSuggestion) => item.excerpt.trim().toLowerCase() !== item.value.trim().toLowerCase();
// "Enhance with AI" only fills fields that hold nothing, judged from the form
// as the user sees it: a field typed into but not saved counts as filled, and
// Links counts as filled as soon as it holds any link, so suggestions never
// overwrite or extend what is already there.
const CONTACT_ENRICH_FIELDS:ContactFieldSuggestion["field"][]=["displayName","role","company","location","bio","link"];
const enrichFieldIsEmpty=(draft:SaveContactRequest,field:ContactFieldSuggestion["field"])=>field==="link"?draft.links.length===0:!(draft[field]??"").trim();
// The rail is the contact page's only home for history, so it shows more rows
// than the email sidebar before "Show more".
const CONTACT_RECENT_EMAIL_ROWS = 8;
const photoUrl = (value:string|null) => value ? `data:image/jpeg;base64,${value}` : null;
async function encodePhoto(file:File):Promise<string>{
  const bitmap=await createImageBitmap(file); const scale=Math.min(1,160/Math.max(bitmap.width,bitmap.height));
  const canvas=document.createElement("canvas"); canvas.width=Math.max(1,Math.round(bitmap.width*scale));canvas.height=Math.max(1,Math.round(bitmap.height*scale));
  const context=canvas.getContext("2d"); if(!context)throw new Error("Could not read the selected photo"); context.drawImage(bitmap,0,0,canvas.width,canvas.height);bitmap.close();
  let quality=.82;let data=canvas.toDataURL("image/jpeg",quality).split(",")[1]??"";
  while(data.length>87_000&&quality>.35){quality-=.1;data=canvas.toDataURL("image/jpeg",quality).split(",")[1]??"";}
  if(data.length>87_000)throw new Error("Photo is too large to save after resizing"); return data;
}

export function ContactsWorkspace({onOpenThread,onSaved,initialContactId=null,accountId=null,initialView="all",view:controlledView,onViewChange,onKeepInTouchChanged}:{onOpenThread(id:string):void;onSaved():void;initialContactId?:string|null;accountId?:string|null;initialView?:ContactsView;view?:ContactsView;onViewChange?(view:ContactsView):void;onKeepInTouchChanged?():void}){
  // The view is controlled when the app owns it (so Tab can switch it and it
  // is remembered); standalone renders fall back to local state.
  const [localView,setLocalView]=useState<ContactsView>(initialView);const view=controlledView??localView;const changeView=(next:ContactsView)=>{setLocalView(next);onViewChange?.(next);};const [kitProfiles,setKitProfiles]=useState<ContactProfile[]>([]);const [selecting,setSelecting]=useState(false);const [checkedIds,setCheckedIds]=useState<string[]>([]);const [bulkNotice,setBulkNotice]=useState<string|null>(null);
  const [merging,setMerging]=useState(false);const [profileRevision,setProfileRevision]=useState(0);
  const [query,setQuery]=useState("");const [profiles,setProfiles]=useState<ContactProfile[]>([]);const [selectedId,setSelectedId]=useState<string|null>(null);const [profile,setProfile]=useState<ContactProfile|null>(null);
  const [timeline,setTimeline]=useState<ContactTimelineItem[]>([]);const [timelineHasMore,setTimelineHasMore]=useState(false);const [draft,setDraft]=useState<SaveContactRequest>(empty());const [loading,setLoading]=useState(true);const [busy,setBusy]=useState(false);const [error,setError]=useState<string|null>(null);const [adding,setAdding]=useState(false);const [confirmDelete,setConfirmDelete]=useState(false);const [suggestions,setSuggestions]=useState<ContactFieldSuggestion[]>([]);const [enrichNotice,setEnrichNotice]=useState<{text:string;failed:boolean}|null>(null);const [enriching,setEnriching]=useState(false);const [moreEmailsAvailable,setMoreEmailsAvailable]=useState(false);const [emailsReviewed,setEmailsReviewed]=useState(0);const [revealedFields,setRevealedFields]=useState<OptionalField[]>([]);
  const [groups,setGroups]=useState<ContactGroup[]>([]);const [selectedGroupId,setSelectedGroupId]=useState<string|null>(null);const [bookProfiles,setBookProfiles]=useState<ContactProfile[]>([]);const [groupPicker,setGroupPicker]=useState<{contactIds:string[];source:"profile"|"bulk"}|null>(null);const [creatingGroup,setCreatingGroup]=useState(false);
  const searchRef=useRef<HTMLInputElement>(null);
  const editorRef=useRef<HTMLDivElement>(null);
  const focusFieldRef=useRef<OptionalField|null>(null);
  const selectedItemRef=useRef<HTMLButtonElement|null>(null);
  const requestedContactId=useRef(initialContactId);
  const preserveSuggestionsForId=useRef<string|null>(null);
  const preserveDraftForId=useRef<string|null>(null);
  const aiEnabled=readAiFeatures().contactEnrichment;
  const load=useCallback(async(keepProfile?:ContactProfile)=>{setLoading(true);setError(null);try{let result=await mailClient.listContactProfiles(query,500,accountId??undefined);const requestedId=requestedContactId.current;if(requestedId&&!query.trim()){requestedContactId.current=null;if(!result.some(item=>item.id===requestedId)){const requestedProfile=await mailClient.getContactProfile(requestedId);if(requestedProfile)result=[requestedProfile,...result];}}if(keepProfile&&!result.some(item=>item.id===keepProfile.id)&&!query.trim())result=[keepProfile,...result];setProfiles(result);setSelectedId(current=>keepProfile?.id??(current&&result.some(item=>item.id===current)?current:requestedId&&result.some(item=>item.id===requestedId)?requestedId:result[0]?.id??null));}catch(reason){setError(errorMessage(reason));}finally{setLoading(false);}},[query,accountId]);
  useEffect(()=>{const timer=setTimeout(()=>void load(),120);return()=>clearTimeout(timer);},[load]);
  // Reminders and birthdays are shared across accounts, like saved profiles.
  const loadKeepInTouch=useCallback(async()=>{try{setKitProfiles(await mailClient.listKeepInTouch());}catch(reason){setError(errorMessage(reason));}},[]);
  useEffect(()=>{void loadKeepInTouch();},[loadKeepInTouch]);
  // Groups are shared across accounts, like saved profiles.
  const loadGroups=useCallback(async()=>{try{const result=await mailClient.listContactGroups();setGroups(result);setSelectedGroupId(current=>current&&result.some(item=>item.id===current)?current:result[0]?.id??null);}catch(reason){setError(errorMessage(reason));}},[]);
  useEffect(()=>{void loadGroups();},[loadGroups]);
  // A group's members can come from any account, so the Groups view reads the whole address book.
  const loadBookProfiles=useCallback(async()=>{try{setBookProfiles(await mailClient.listContactProfiles("",5000));}catch(reason){setError(errorMessage(reason));}},[]);
  useEffect(()=>{if(view==="groups")void loadBookProfiles();},[view,loadBookProfiles]);
  const groupChanged=(next:ContactGroup)=>setGroups(current=>[...current.filter(item=>item.id!==next.id),next].sort(byGroupName));
  const runGroupAction=async<T,>(work:()=>Promise<T>):Promise<T|null>=>{setBusy(true);setError(null);try{return await work();}catch(reason){setError(errorMessage(reason));return null;}finally{setBusy(false);}};
  const createGroup=async(name:string)=>{const created=await runGroupAction(()=>mailClient.createContactGroup(name));if(!created)return false;groupChanged(created);setSelectedGroupId(created.id);return true;};
  const renameGroup=async(id:string,name:string)=>{const renamed=await runGroupAction(()=>mailClient.renameContactGroup(id,name));if(renamed)groupChanged(renamed);return !!renamed;};
  const deleteGroup=async(id:string)=>{const deleted=await runGroupAction(async()=>{await mailClient.deleteContactGroup(id);return true;});if(!deleted)return;setGroups(current=>{const rest=current.filter(item=>item.id!==id);setSelectedGroupId(rest[0]?.id??null);return rest;});};
  const removeFromGroup=async(id:string,contactId:string)=>{const next=await runGroupAction(()=>mailClient.removeContactGroupMembers(id,[contactId]));if(next)groupChanged(next);};
  // Adding a mail-derived contact or a typed address saves a contact, so the lists refresh.
  const addToGroup=async(id:string,contactIds:string[],emails:string[])=>{const next=await mailClient.addContactGroupMembers(id,contactIds,emails);groupChanged(next);if(emails.length||contactIds.some(item=>item.startsWith("derived:"))){void loadBookProfiles();void load();}};
  const pickGroup=async(groupId:string|null,name="")=>{
    if(!groupPicker)return;const ids=groupPicker.contactIds;
    const next=groupId?await mailClient.addContactGroupMembers(groupId,ids):await mailClient.createContactGroup(name,ids);groupChanged(next);
    if(groupPicker.source==="bulk"){setBulkNotice(`Added ${ids.length} ${ids.length===1?"contact":"contacts"} to ${next.name}`);stopSelecting();}
    if(!ids.some(item=>item.startsWith("derived:")))return;
    void load();
    // The open profile was mail-derived: show it under its new saved id.
    if(profile?.id.startsWith("derived:")&&ids.includes(profile.id)){const owners=await mailClient.resolveContactIds(profile.addresses);const savedId=owners[profile.addresses[0]];const saved=savedId?await mailClient.getContactProfile(savedId):null;if(saved)onSectionChanged(saved);}
  };
  const keepInTouchChanged=(updated:ContactProfile[],previousId?:string|null)=>{
    setProfiles(current=>{const ids=new Set([...updated.map(item=>item.id),...(previousId?[previousId]:[])]);const kept=current.filter(item=>!ids.has(item.id)&&!updated.some(next=>next.addresses.some(address=>item.addresses.includes(address))));return [...kept,...updated.map(next=>{const prior=current.find(item=>item.id===next.id||item.id===previousId);return prior?{...next,sentCount:prior.sentCount,receivedCount:prior.receivedCount,lastInteractedAt:prior.lastInteractedAt}:next;})];});
    void loadKeepInTouch();onKeepInTouchChanged?.();
  };
  const onSectionChanged=(updated:ContactProfile)=>{
    const previousId=profile?.id??null;setProfile(updated);keepInTouchChanged([updated],previousId);
    // Setting a reminder on a mail-derived contact saves it under a new id.
    if(updated.id!==selectedId){setDraft(current=>({...current,id:updated.id}));preserveDraftForId.current=updated.id;preserveSuggestionsForId.current=updated.id;setSelectedId(updated.id);}
  };
  const toggleChecked=(id:string)=>setCheckedIds(current=>current.includes(id)?current.filter(value=>value!==id):[...current,id]);
  const stopSelecting=()=>{setSelecting(false);setCheckedIds([]);};
  // Bulk selection exists only in All Contacts.
  useEffect(()=>{if(view!=="all"){setSelecting(false);setCheckedIds([]);}},[view]);
  const applyBulkFrequency=async(value:string)=>{if(!value||!checkedIds.length)return;const days=value==="off"?null:Number(value);setBusy(true);setError(null);setBulkNotice(null);try{const updated=await mailClient.setKeepInTouch(checkedIds,days);keepInTouchChanged(updated);setBulkNotice(days===null?`Keep in touch turned off for ${updated.length} ${updated.length===1?"contact":"contacts"}`:`${frequencyLabel(days)} for ${updated.length} ${updated.length===1?"contact":"contacts"}`);stopSelecting();if(selectedId&&!updated.some(item=>item.id===selectedId)){const replaced=updated.find(item=>profile?.addresses.some(address=>item.addresses.includes(address)));if(replaced)setSelectedId(replaced.id);}else if(selectedId){const current=updated.find(item=>item.id===selectedId);if(current)setProfile(current);}}catch(reason){setError(errorMessage(reason));}finally{setBusy(false);}};
  useEffect(()=>{const onKey=(event:KeyboardEvent)=>{if(event.key!=="/"||event.metaKey||event.ctrlKey||event.altKey)return;const target=event.target;if(target instanceof HTMLElement&&(target.isContentEditable||["INPUT","TEXTAREA","SELECT"].includes(target.tagName)))return;event.preventDefault();searchRef.current?.focus();};window.addEventListener("keydown",onKey);return()=>window.removeEventListener("keydown",onKey);},[]);
  const addingRef=useRef(adding);
  useEffect(()=>{addingRef.current=adding;},[adding]);
  useEffect(()=>{if(!selectedId){setProfile(null);setTimeline([]);setTimelineHasMore(false);if(!addingRef.current)setDraft(empty());return;}const preserveSuggestions=preserveSuggestionsForId.current===selectedId;preserveSuggestionsForId.current=null;const preserveDraft=preserveDraftForId.current===selectedId;preserveDraftForId.current=null;let active=true;void Promise.all([mailClient.getContactProfile(selectedId),mailClient.contactTimeline(selectedId,0,20,accountId??undefined)]).then(([item,events])=>{if(!active)return;setProfile(item);setTimeline(events);setTimelineHasMore(events.length===20);if(!preserveDraft){setDraft(item?draftFrom(item):empty());setRevealedFields([]);}setAdding(false);if(!preserveSuggestions){setSuggestions([]);setMoreEmailsAvailable(false);setEmailsReviewed(0);setEnrichNotice(null);}}).catch(reason=>{if(active)setError(errorMessage(reason));});return()=>{active=false;};},[selectedId,accountId,profileRevision]);
  const setField=(field:keyof SaveContactRequest,value:unknown)=>setDraft(current=>({...current,[field]:value}));
  const save=async()=>{setBusy(true);setError(null);try{const saved=await mailClient.saveContactProfile({...draft,addresses:draft.addresses.map(value=>value.trim()).filter(Boolean),links:draft.links.map(value=>value.trim()).filter(Boolean),birthday:draft.birthday?.trim()||null});setProfile(saved);setDraft({...saved});setSelectedId(saved.id);setAdding(false);onSaved();await load(saved);void loadKeepInTouch();}catch(reason){setError(errorMessage(reason));}finally{setBusy(false);}};
  const toggleFavorite=async()=>{
    if(!profile){setField("favorite",!draft.favorite);return;}
    setBusy(true);setError(null);
    try{
      const saved=await mailClient.saveContactProfile({...profile,favorite:!profile.favorite});
      setProfile(saved);
      setDraft(current=>({...current,id:saved.id,favorite:saved.favorite}));
      setProfiles(current=>[...current.filter(item=>item.id!==profile.id&&item.id!==saved.id),{...saved,sentCount:current.find(item=>item.id===profile.id)?.sentCount??saved.sentCount,receivedCount:current.find(item=>item.id===profile.id)?.receivedCount??saved.receivedCount,lastInteractedAt:current.find(item=>item.id===profile.id)?.lastInteractedAt??saved.lastInteractedAt}]);
      if(saved.id!==selectedId){preserveDraftForId.current=saved.id;preserveSuggestionsForId.current=saved.id;setSelectedId(saved.id);}
    }catch(reason){setError(errorMessage(reason));}finally{setBusy(false);}
  };
  const discard=()=>{if(adding){setAdding(false);setDraft(empty());return;}if(profile)setDraft(draftFrom(profile));};
  const revealField=(field:OptionalField)=>{focusFieldRef.current=field;setRevealedFields(current=>current.includes(field)?current:[...current,field]);};
  useEffect(()=>{const field=focusFieldRef.current;if(!field)return;focusFieldRef.current=null;editorRef.current?.querySelector<HTMLElement>(`[data-contact-field="${field}"] input,[data-contact-field="${field}"] textarea`)?.focus();},[revealedFields]);
  const startNewGroup=()=>{changeView("groups");setCreatingGroup(true);};
  const startNew=()=>{if(view==="groups")changeView("all");setSelectedId(null);setProfile(null);setDraft(empty());setAdding(true);setConfirmDelete(false);setSuggestions([]);setMoreEmailsAvailable(false);setEmailsReviewed(0);};
  const deleteProfile=async()=>{if(!profile||profile.id.startsWith("derived:"))return;if(!confirmDelete){setConfirmDelete(true);return;}try{await mailClient.deleteContactProfile(profile.id);setConfirmDelete(false);setSelectedId(null);setProfile(null);setAdding(false);await load();void loadGroups();}catch(reason){setError(errorMessage(reason));}};
  const enrich=async(searchMore=false)=>{if(!selectedId)return;const emptyFields=CONTACT_ENRICH_FIELDS.filter(field=>enrichFieldIsEmpty(draft,field));if(!emptyFields.length){setEnrichNotice({text:"Every contact field already has a value.",failed:true});return;}setEnriching(true);setEnrichNotice(null);try{const config=readAiRequestConfig("enhancing a contact","contactEnrichment");const result=await mailClient.enrichContact(selectedId,config.provider,config.model,config.endpoint,emptyFields,searchMore,accountId??undefined,config.reasoning);if(searchMore){setSuggestions(current=>[...current,...result.suggestions.filter(item=>!current.some(existing=>existing.field===item.field&&existing.value===item.value))]);setEmailsReviewed(current=>current+result.messagesReviewed);if(!result.suggestions.length)setEnrichNotice({text:"No additional profile details were found.",failed:false});}else{setSuggestions(result.suggestions);setEmailsReviewed(result.messagesReviewed);if(!result.suggestions.length)setEnrichNotice({text:"No supported profile details were found in the available emails.",failed:false});}setMoreEmailsAvailable(result.hasMore);}catch(reason){setEnrichNotice({text:errorMessage(reason),failed:true});}finally{setEnriching(false);}};
  const applySuggestion=async(item:ContactFieldSuggestion)=>{if(!enrichFieldIsEmpty(draft,item.field))return;const next={...draft};if(item.field==="link")next.links=[...new Set([...next.links,item.value])];else next[item.field]=item.value;setDraft(next);setBusy(true);try{const saved=await mailClient.saveContactProfile({...next,id:profile?.id??selectedId});setProfile(saved);setDraft({...saved});setSuggestions(current=>current.filter(value=>value!==item));if(saved.id!==selectedId)preserveSuggestionsForId.current=saved.id;setSelectedId(saved.id);await load(saved);}catch(reason){setError(errorMessage(reason));}finally{setBusy(false);}};
  const loadOlder=async()=>{if(!selectedId)return;try{const next=await mailClient.contactTimeline(selectedId,timeline.length,20,accountId??undefined);setTimeline(current=>[...current,...next]);setTimelineHasMore(next.length===20);}catch(reason){setError(errorMessage(reason));}};
  const avatar=photoUrl(draft.photoData);const initial=(draft.displayName||draft.addresses[0]||"?").trim().slice(0,1).toLocaleUpperCase();
  const scopedProfile=profiles.find(item=>item.id===profile?.id)??profile;
  const lastContact=scopedProfile&&profile?lastTouchAt({lastInteractedAt:scopedProfile.lastInteractedAt,keepInTouch:profile.keepInTouch}):null;
  const unsaved=adding||(!!profile&&formSnapshot(draft)!==formSnapshot(draftFrom(profile)));
  const showField=(field:OptionalField)=>adding||revealedFields.includes(field)||optionalFieldHasValue(draft,field)||(!!profile&&optionalFieldHasValue(draftFrom(profile),field));
  const hiddenFields=OPTIONAL_FIELDS.filter(item=>!showField(item.field));
  // Reference material and AI help sit in a rail beside the form, like the
  // context panel beside an open email.
  const showRail=!!profile&&(aiEnabled||timeline.length>0);
  // A suggestion disappears the moment its field is filled in, so one can
  // never overwrite typing the user did while enrichment was running.
  const visibleSuggestions=suggestions.filter(item=>enrichFieldIsEmpty(draft,item.field));
  const orderedProfiles=useMemo(()=>[...profiles].sort((a,b)=>Number(b.favorite)-Number(a.favorite)||Date.parse(b.lastInteractedAt??"")-Date.parse(a.lastInteractedAt??"")||(a.displayName||a.addresses[0]||"").localeCompare(b.displayName||b.addresses[0]||"",undefined,{sensitivity:"base"})),[profiles]);
  const favoriteProfiles=orderedProfiles.filter(item=>item.favorite);const recentProfiles=orderedProfiles.filter(item=>!item.favorite);
  const needle=query.trim().toLocaleLowerCase();
  const kitGroups=useMemo(()=>{const now=new Date();const visible=kitProfiles.filter(item=>matchesQuery(item,needle));return {
    reminders:KEEP_IN_TOUCH_GROUPS.map(group=>({...group,items:visible.filter(item=>keepInTouchStatus(item.keepInTouchDueAt,now)===group.status)})).filter(group=>group.items.length),
    birthdays:visible.flatMap(item=>{const next=item.birthday?nextBirthday(item.birthday,now):null;return next&&next.daysAway<=UPCOMING_BIRTHDAY_DAYS?[{item,daysAway:next.daysAway}]:[];}).sort((a,b)=>a.daysAway-b.daysAway).map(entry=>entry.item),
  };},[kitProfiles,needle]);
  const dueCount=kitProfiles.filter(item=>isKeepInTouchDue(item)).length;
  const navigableProfiles=useMemo(()=>view==="all"?orderedProfiles:view==="groups"?[]:[...new Map([...kitGroups.reminders.flatMap(group=>group.items),...kitGroups.birthdays].map(item=>[item.id,item])).values()],[view,orderedProfiles,kitGroups]);
  useEffect(()=>{const onKey=(event:KeyboardEvent)=>{if(event.key!=="ArrowDown"&&event.key!=="ArrowUp")return;if(event.metaKey||event.ctrlKey||event.altKey)return;const target=event.target;if(target instanceof HTMLElement&&(target.isContentEditable||["INPUT","TEXTAREA","SELECT"].includes(target.tagName)))return;if(!navigableProfiles.length)return;event.preventDefault();const currentIndex=navigableProfiles.findIndex(item=>item.id===selectedId);const nextIndex=event.key==="ArrowDown"?Math.min(navigableProfiles.length-1,currentIndex+1):Math.max(0,currentIndex===-1?0:currentIndex-1);const next=navigableProfiles[nextIndex];if(!next)return;setAdding(false);setSelectedId(next.id);};window.addEventListener("keydown",onKey);return()=>window.removeEventListener("keydown",onKey);},[navigableProfiles,selectedId]);
  const visibleGroups=useMemo(()=>filterGroups(groups,needle),[groups,needle]);
  const selectedGroup=groups.find(item=>item.id===selectedGroupId)??null;
  const profileGroups=profile?groupsForContact(groups,profile.id):[];
  useEffect(()=>{if(view!=="groups")return;const onKey=(event:KeyboardEvent)=>{if(event.key!=="ArrowDown"&&event.key!=="ArrowUp")return;if(event.metaKey||event.ctrlKey||event.altKey)return;const target=event.target;if(target instanceof HTMLElement&&(target.isContentEditable||["INPUT","TEXTAREA","SELECT"].includes(target.tagName)))return;if(!visibleGroups.length)return;event.preventDefault();const currentIndex=visibleGroups.findIndex(item=>item.id===selectedGroupId);const nextIndex=event.key==="ArrowDown"?Math.min(visibleGroups.length-1,currentIndex+1):Math.max(0,currentIndex===-1?0:currentIndex-1);const next=visibleGroups[nextIndex];if(next)setSelectedGroupId(next.id);};window.addEventListener("keydown",onKey);return()=>window.removeEventListener("keydown",onKey);},[view,visibleGroups,selectedGroupId]);
  const openContact=(id:string)=>{changeView("all");setAdding(false);setSelectedId(id);};
  const openGroup=(id:string)=>{changeView("groups");setSelectedGroupId(id);};
  useEffect(()=>{selectedItemRef.current?.scrollIntoView?.({block:"nearest"});},[selectedId]);
  const listSize=useContactListWidth();
  return <section className="contacts-workspace" aria-label="Contacts">
    {merging?<MergeContactsDialog contacts={profiles.filter(item=>checkedIds.includes(item.id))} onClose={()=>setMerging(false)} onMerged={merged=>{setMerging(false);setProfileRevision(current=>current+1);stopSelecting();setProfile(merged);setDraft(draftFrom(merged));setSelectedId(merged.id);void load(merged);void loadKeepInTouch();void loadGroups();onSaved();}}/>:null}
    <header className="contacts-header"><div><span className="eyebrow">Address book <span className="eyebrow-account">· {accountId??"All accounts"}</span></span><h1>Contacts</h1></div><div className="contacts-header-actions"><ContactManagement hasUnsavedChanges={unsaved} addresses={profile?.addresses??[]} onChanged={()=>{void load();void loadKeepInTouch();void loadGroups();onSaved();}}/><button type="button" className="btn" onClick={startNewGroup}><Users size={ICON_SIZE.md}/>New Group</button><button type="button" className="btn" onClick={startNew}><Plus size={ICON_SIZE.md}/>New Contact</button></div></header>
    <div className="contacts-workspace-body" style={{ "--contact-list-width": `${listSize.width}px` } as CSSProperties}>
      <div className="contacts-list-pane"><PanelResizeHandle {...listSize} label="Resize contact list" controlsId="contact-list-panel" title="Drag to resize the contact list. Use arrow keys to adjust; double-click to reset."/>
      <aside id="contact-list-panel" className="contacts-list-panel" aria-label="Contact list">
        <div className="segmented contacts-view-switch" role="tablist" aria-label="Contact Views"><button type="button" className="segment" role="tab" aria-selected={view==="all"} data-mailbox-tab-shortcut onClick={()=>changeView("all")}>All Contacts</button><button type="button" className="segment" role="tab" aria-selected={view==="keepInTouch"} data-mailbox-tab-shortcut onClick={()=>changeView("keepInTouch")}>Keep in Touch{dueCount?<span className="contacts-view-count" aria-label={`${dueCount} due`}>{dueCount}</span>:null}</button><button type="button" className="segment" role="tab" aria-selected={view==="groups"} data-mailbox-tab-shortcut onClick={()=>changeView("groups")}>Groups</button></div>
        <label className="contacts-search"><Search size={ICON_SIZE.md}/><input ref={searchRef} {...SEARCH_INPUT_ATTRIBUTES} aria-label={view==="groups"?"Search groups":"Search contacts"} placeholder={view==="groups"?"Search groups":"Search contacts"} value={query} data-mailbox-tab-shortcut onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key!=="Escape")return;event.preventDefault();event.stopPropagation();setQuery("");if(selectedItemRef.current)selectedItemRef.current.focus();else event.currentTarget.blur();}}/><kbd>/</kbd></label>
        {view==="all"?<div className="contacts-list-toolbar"><p className="contacts-sort-hint">Favorites first · then recent activity</p><button type="button" className="btn-link contacts-select-toggle" aria-pressed={selecting} onClick={()=>selecting?stopSelecting():setSelecting(true)}>{selecting?"Done":"Select"}</button></div>:view==="keepInTouch"?<p className="contacts-sort-hint">Soonest reminder first · email either way counts as contact</p>:null}
        {selecting?<div className="contacts-bulk-bar"><span>{checkedIds.length} selected</span><select aria-label="Keep in Touch Frequency" value="" disabled={busy||!checkedIds.length} onChange={event=>void applyBulkFrequency(event.target.value)}><option value="" disabled>Keep in Touch…</option>{KEEP_IN_TOUCH_FREQUENCIES.map(item=><option key={item.days} value={item.days}>{item.label}</option>)}<option value="off">Off</option></select><button type="button" className="btn btn-sm" disabled={busy||!checkedIds.length} onClick={()=>setGroupPicker({contactIds:checkedIds,source:"bulk"})}>Add to Group…</button><button type="button" className="btn btn-sm" disabled={busy||unsaved||checkedIds.length<2||checkedIds.length>51||checkedIds.some(id=>id.startsWith("derived:"))} title="Select 2 to 51 saved contacts and save any edits first" onClick={()=>setMerging(true)}>Merge…</button></div>:null}
        {bulkNotice?<p className="contacts-status" role="status">{bulkNotice}</p>:null}
        {loading&&view==="all"?<p className="contacts-status">Loading contacts…</p>:null}{error?<p className="contacts-error" role="alert">{error}</p>:null}
        {view==="all"?<div className="contacts-list">{favoriteProfiles.length?<section className="contact-list-group" aria-label="Favorites"><h2>Favorites</h2>{favoriteProfiles.map(item=><ContactListItem key={item.id} item={item} selected={selectedId===item.id&&!adding} itemRef={selectedId===item.id&&!adding?selectedItemRef:undefined} checked={selecting?checkedIds.includes(item.id):undefined} onToggleChecked={()=>toggleChecked(item.id)} onSelect={()=>{setAdding(false);setSelectedId(item.id);}}/>)}</section>:null}{recentProfiles.length?<section className="contact-list-group" aria-label="Recent contacts"><h2>Recent</h2>{recentProfiles.map(item=><ContactListItem key={item.id} item={item} selected={selectedId===item.id&&!adding} itemRef={selectedId===item.id&&!adding?selectedItemRef:undefined} checked={selecting?checkedIds.includes(item.id):undefined} onToggleChecked={()=>toggleChecked(item.id)} onSelect={()=>{setAdding(false);setSelectedId(item.id);}}/>)}</section>:null}</div>
        :view==="groups"?<ContactGroupList groups={visibleGroups} selectedId={selectedGroupId} busy={busy} searching={!!needle} creating={creatingGroup} onCreatingChange={setCreatingGroup} onSelect={setSelectedGroupId} onCreate={createGroup}/>
        :<div className="contacts-list">{kitGroups.reminders.map(group=><section key={group.status} className={`contact-list-group kit-${group.status}`} aria-label={group.label}><h2>{group.label}</h2>{group.items.map(item=><ContactListItem key={item.id} item={item} detail={item.keepInTouch.intervalDays!==null&&item.keepInTouchDueAt?`${frequencyLabel(item.keepInTouch.intervalDays)} · ${describeDue(item.keepInTouchDueAt)}`:undefined} selected={selectedId===item.id&&!adding} itemRef={selectedId===item.id&&!adding?selectedItemRef:undefined} onSelect={()=>{setAdding(false);setSelectedId(item.id);}}/>)}</section>)}{kitGroups.birthdays.length?<section className="contact-list-group" aria-label="Upcoming Birthdays"><h2>Upcoming Birthdays</h2>{kitGroups.birthdays.map(item=><ContactListItem key={item.id} item={item} detail={describeBirthday(item.birthday??"")} showDue={false} selected={selectedId===item.id&&!adding} onSelect={()=>{setAdding(false);setSelectedId(item.id);}}/>)}</section>:null}</div>}
        {view==="all"&&!loading&&profiles.length===0?<p className="contacts-empty">No contacts found. People you email will appear here.</p>:null}
        {view==="keepInTouch"&&!kitGroups.reminders.length&&!kitGroups.birthdays.length?<p className="contacts-empty">{needle?"No reminders match this search.":"No keep-in-touch reminders yet. Choose a frequency on a contact, or use Select in All Contacts to set one for several people at once."}</p>:null}
      </aside></div>
      <section className="contact-profile-panel" aria-label="Contact details">
        {view==="groups"?selectedGroup?<div className="contact-profile-layout"><div className="contact-profile-main"><div className="contact-profile-column"><ContactGroupDetail key={selectedGroup.id} group={selectedGroup} profiles={bookProfiles} busy={busy} onOpenContact={openContact} onRename={name=>renameGroup(selectedGroup.id,name)} onDelete={()=>deleteGroup(selectedGroup.id)} onAddMembers={(contactIds,emails)=>addToGroup(selectedGroup.id,contactIds,emails)} onRemoveMember={contactId=>removeFromGroup(selectedGroup.id,contactId)}/></div></div></div>
        :<div className="contacts-empty-state"><ContactRound size={ICON_SIZE.display}/><p>{groups.length?"Select a group to see its members":"Create a group to email several people at once"}</p></div>
        :adding||profile?<div className={`contact-profile-layout${showRail?" has-rail":""}`}><div className="contact-profile-main"><div className="contact-profile-column">
          <div className="contact-profile-top"><div className="contact-identity"><label className="contact-avatar large" title="Contact photo">{avatar?<img src={avatar} alt=""/>:<span>{initial||<UserRound/>}</span>}<input type="file" accept="image/avif,image/gif,image/jpeg,image/png,image/webp" aria-label="Upload contact photo" onChange={async event=>{const file=event.target.files?.[0];if(!file)return;try{setField("photoData",await encodePhoto(file));}catch(reason){setError(errorMessage(reason));}}}/></label><div><input className="contact-name-input" aria-label="Name" placeholder="Name" value={draft.displayName??""} onChange={event=>setField("displayName",event.target.value)}/><input aria-label="Role" placeholder="Role or title" value={draft.role??""} onChange={event=>setField("role",event.target.value)}/>{scopedProfile?<p className="contact-identity-meta">{[`${scopedProfile.sentCount} sent`,`${scopedProfile.receivedCount} received`,lastContact?`Last contact ${formatKeepInTouchDate(lastContact)}`:null].filter(Boolean).join(" · ")}</p>:null}</div></div>
            <div className="contact-profile-actions">{confirmDelete?<div className="contact-delete-confirm"><span>Delete this profile?</span><button type="button" className="btn btn-sm" onClick={()=>setConfirmDelete(false)}>Cancel</button><button type="button" className="btn btn-sm btn-danger" onClick={()=>void deleteProfile()}>Confirm</button></div>:<><button type="button" className="btn-icon" aria-label="Favorite contact" aria-pressed={draft.favorite} disabled={busy} onClick={()=>void toggleFavorite()}><Heart size={ICON_SIZE.lg} fill={draft.favorite?"currentColor":"none"}/></button>{profile&&!profile.id.startsWith("derived:")?<button type="button" className="btn-icon" aria-label="Delete contact" onClick={()=>void deleteProfile()}><Trash size={ICON_SIZE.lg}/></button>:null}</>}</div></div>
          <div ref={editorRef} className="contact-editor-grid"><ContactAddressField addresses={draft.addresses} disabled={busy} onChange={addresses=>setField("addresses",addresses)}/>
            {showField("company")?<label data-contact-field="company">Company<input value={draft.company??""} onChange={event=>setField("company",event.target.value)}/></label>:null}
            {showField("location")?<label data-contact-field="location">Location<input value={draft.location??""} onChange={event=>setField("location",event.target.value)}/></label>:null}
            {showField("birthday")?<label data-contact-field="birthday">Birthday<input value={draft.birthday??""} placeholder="MM-DD or YYYY-MM-DD" inputMode="numeric" maxLength={10} onChange={event=>setField("birthday",event.target.value)}/></label>:null}
            {showField("links")?<div data-contact-field="links" className="contact-field-slot"><ContactLinksField links={draft.links} disabled={busy} onChange={links=>setField("links",links)}/></div>:null}
            {showField("bio")?<label data-contact-field="bio" className="wide">About<GrowingTextarea value={draft.bio??""} onChange={event=>setField("bio",event.target.value)}/></label>:null}
            {showField("notes")?<label data-contact-field="notes" className="wide">Notes<GrowingTextarea value={draft.notes??""} onChange={event=>setField("notes",event.target.value)}/></label>:null}
            {hiddenFields.length?<div className="contact-add-fields">{hiddenFields.map(item=><button type="button" className="btn btn-sm" key={item.field} aria-label={`Add ${item.label.toLocaleLowerCase()}`} onClick={()=>revealField(item.field)}><Plus size={ICON_SIZE.sm}/>{item.label}</button>)}</div>:null}
          </div>
          {profile?<KeepInTouchSection profile={profile} onChanged={onSectionChanged}/>:null}
          {profile?<ContactGroupsSection groups={profileGroups} busy={busy} onOpenGroup={openGroup} onAdd={()=>setGroupPicker({contactIds:[profile.id],source:"profile"})} onRemove={groupId=>removeFromGroup(groupId,profile.id)}/>:null}
          {unsaved?<div className="contact-save-bar" role="region" aria-label="Save changes"><span>{adding?"New contact":"Unsaved changes"}</span><div><button type="button" className="btn" disabled={busy} onClick={discard}>Discard</button><button type="button" className="btn btn-primary" disabled={busy} onClick={()=>void save()}><Check size={ICON_SIZE.md}/>Save contact</button></div></div>:null}
          </div></div>
          {showRail?<aside className="contact-context-rail" aria-label="Contact context">
            {aiEnabled?<ContactEnrichmentCard suggestions={visibleSuggestions} notice={enrichNotice} enriching={enriching} emailsReviewed={emailsReviewed} moreAvailable={moreEmailsAvailable} onEnrich={searchMore=>void enrich(searchMore)} onApply={item=>void applySuggestion(item)} onOpenThread={onOpenThread}/>:null}
            {profile?<ContactFilesSection key={`${profile.id}:${profileRevision}`} contactId={profile.id} onShowMessage={threadId=>onOpenThread(threadId)}/>:null}
            {timeline.length?<RecentEmailsSection items={timeline} limit={CONTACT_RECENT_EMAIL_ROWS} onOpenThread={onOpenThread} onLoadOlder={timelineHasMore?()=>void loadOlder():undefined}/>:null}
          </aside>:null}
        </div>:<div className="contacts-empty-state"><ContactRound size={ICON_SIZE.display}/><p>Select a contact to see their details</p></div>}
      </section>
    </div>
    {groupPicker?<ContactGroupPicker title={groupPicker.source==="bulk"?`Add ${groupPicker.contactIds.length} ${groupPicker.contactIds.length===1?"contact":"contacts"} to a group`:"Add to a group"} groups={groups} currentIds={groups.filter(group=>groupPicker.contactIds.every(id=>group.memberIds.includes(id))).map(group=>group.id)} onPick={groupId=>pickGroup(groupId)} onCreate={name=>pickGroup(null,name)} onClose={()=>setGroupPicker(null)}/>:null}
  </section>;
}

function ContactListItem({item,selected,itemRef,onSelect,detail,showDue=true,checked,onToggleChecked}:{item:ContactProfile;selected:boolean;itemRef?:{current:HTMLButtonElement|null};onSelect():void;detail?:string;showDue?:boolean;checked?:boolean;onToggleChecked?():void}){
  const avatar=photoUrl(item.photoData);const name=item.displayName||item.addresses[0];
  const due=showDue&&isKeepInTouchDue(item);
  const content=<><span className="contact-avatar small">{avatar?<img src={avatar} alt=""/>:<span>{(item.displayName||item.addresses[0]||"?").slice(0,1).toLocaleUpperCase()}</span>}</span><span className="contact-list-copy"><strong>{name}</strong><small>{detail??(item.company||item.addresses[0])}</small></span>{due?<CircleAlert className="contact-kit-due" size={ICON_SIZE.sm} aria-label="Due to reconnect"/>:null}{item.favorite?<Heart size={ICON_SIZE.sm} fill="currentColor"/>:null}</>;
  // In selection mode a row is a checkbox, so one click never both selects
  // the row for a bulk change and opens the profile.
  if(checked!==undefined)return <label className={`contact-list-item selectable${checked?" checked":""}`}><input type="checkbox" checked={checked} aria-label={`Select ${name}`} onChange={()=>onToggleChecked?.()}/>{content}</label>;
  return <button ref={itemRef} type="button" data-mailbox-tab-shortcut className={`contact-list-item${selected?" selected":""}`} aria-pressed={selected} onClick={onSelect}>{content}</button>;
}

// Starts at two lines and grows with its content, so an empty About or Notes
// field takes a single row instead of a tall blank box.
function GrowingTextarea(props:TextareaHTMLAttributes<HTMLTextAreaElement>){
  const ref=useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(()=>{const element=ref.current;if(!element)return;element.style.height="auto";element.style.height=`${element.scrollHeight+element.offsetHeight-element.clientHeight}px`;},[props.value]);
  return <textarea ref={ref} rows={2} {...props}/>;
}

/**
 * AI profile suggestions, laid out like the Brief card in the email context
 * panel. Nothing is written until the user picks "Use suggestion".
 */
function ContactEnrichmentCard({suggestions,notice,enriching,emailsReviewed,moreAvailable,onEnrich,onApply,onOpenThread}:{suggestions:ContactFieldSuggestion[];notice:{text:string;failed:boolean}|null;enriching:boolean;emailsReviewed:number;moreAvailable:boolean;onEnrich(searchMore:boolean):void;onApply(item:ContactFieldSuggestion):void;onOpenThread(id:string):void}){
  const headingId=useId();
  return <section className="context-section contact-enrichment" aria-labelledby={headingId}>
    <ContextSectionHeader title="Profile Suggestions" titleId={headingId} actions={<button type="button" className="btn btn-sm thread-assist-run" disabled={enriching} onClick={()=>onEnrich(false)}>{enriching?<LoaderCircle className="spin" size={ICON_SIZE.xs}/>:<Sparkles size={ICON_SIZE.xs}/>}Enhance with AI</button>}/>
    {enriching?<p className="context-status">Reading emails…</p>:!notice&&!suggestions.length&&emailsReviewed===0?<p className="context-status">Fill empty fields from your emails with this person. Nothing changes until you use a suggestion.</p>:null}
    {notice?<p className={notice.failed?"context-status contacts-error":"context-status"} role={notice.failed?"alert":"status"}>{notice.text}</p>:null}
    {suggestions.map((item,index)=><article key={`${item.field}-${index}`} className="contact-suggestion">
      <span className="contact-suggestion-field">{CONTACT_SUGGESTION_FIELD_LABELS[item.field]}</span>
      <strong>{item.value}</strong>
      {suggestionHasDistinctEvidence(item)?<div className="contact-suggestion-evidence"><span>From the email</span><blockquote>{item.excerpt}</blockquote></div>:null}
      <div className="contact-suggestion-actions"><button type="button" className="btn btn-sm" onClick={()=>onApply(item)}>Use suggestion</button><button type="button" className="btn-link context-link-button" onClick={()=>onOpenThread(item.sourceThreadId)}>View source email</button></div>
    </article>)}
    {emailsReviewed>0?<p className="context-section-note">{emailsReviewed} {emailsReviewed===1?"email":"emails"} reviewed</p>:null}
    {moreAvailable?<button type="button" className="btn-link context-link-button" disabled={enriching} onClick={()=>onEnrich(true)}>Search more emails</button>:null}
  </section>;
}
