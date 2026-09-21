export default function AttachmentList(props) {
  return (
    <ul class="flex flex-wrap gap-2">
      {!props.empty && props.items.map(item => <li class="relative"><input type="hidden" name="attachments" value={item.path} /><img src={item.previewUrl} alt={item.name} class="w-20 h-20 object-cover rounded border border-gray-300 dark:border-gray-700" /><button type="button" aria-label="Remove" class="absolute top-0.5 right-0.5 bg-black/70 text-white text-xs rounded w-5 h-5 flex items-center justify-center hover:bg-black" onclick="this.closest('li').remove()">x</button></li>)}
    </ul>
  );
}
