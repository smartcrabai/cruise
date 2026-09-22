export default function EmptyState(props) {
  return (
    <div class="h-full flex items-center justify-center">
      <p class="text-gray-500 dark:text-gray-400 text-sm">{props.message}</p>
    </div>
  );
}
