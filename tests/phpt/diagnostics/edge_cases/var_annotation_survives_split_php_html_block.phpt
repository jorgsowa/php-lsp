===description===
A Yii2-style view template with `/** @var Post $model */` immediately
followed by `?>` (closing the PHP block), then inline HTML, then a new
`<?php` block that uses `$model`. mir's `find_preceding_docblock` now
skips a closing `?>`/`InlineHtml`/re-opening `<?php` gap when locating
the preceding docblock, so the annotation still attaches.
===file===
<?php
class Post { public string $title = ''; }
/** @var Post $model */
?>
<div>
<?php if ($model->title !== ''): ?>
    <h1><?= $model->title ?></h1>
<?php endif; ?>
</div>
