export default function ErrorPage(props) {
  return (
    <div class="h-full flex items-center justify-center">
      <p class="text-red-600 dark:text-red-400 text-sm">{props.message}</p>
    </div>
  );
}
