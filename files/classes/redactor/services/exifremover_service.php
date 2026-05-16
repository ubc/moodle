<?php
// This file is part of Moodle - http://moodle.org/
//
// Moodle is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Moodle is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Moodle.  If not, see <http://www.gnu.org/licenses/>.

namespace core_files\redactor\services;

use admin_setting_configcheckbox;
use admin_setting_configexecutable;
use admin_setting_configselect;
use admin_setting_configtextarea;
use admin_setting_heading;
use core\exception\moodle_exception;
use core\output\html_writer;

/**
 * Remove EXIF data from supported image files using PHP GD, or ExifTool if it is configured.
 *
 * The PHP GD stripping has minimal configuration and removes all EXIF data.
 * More stripping is made available when using ExifTool.
 *
 * @package   core_files
 * @copyright Meirza <meirza.arson@moodle.com>
 * @license   http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class exifremover_service extends service implements file_redactor_service_interface {
    /** @var array REMOVE_TAGS Tags to remove and their corresponding values. */
    const REMOVE_TAGS = [
        "gps" => '"-gps*="',
        "all" => "-all=",
    ];

    /** @var string DEFAULT_REMOVE_TAGS Default tags that will be removed. */
    const DEFAULT_REMOVE_TAGS = "gps";

    /** @var string DEFAULT_MIMETYPE Default MIME type for images. */
    const DEFAULT_MIMETYPE = "image/jpeg";

    /**
     * PRESERVE_TAGS Tag to preserve when stripping EXIF data.
     *
     * To add a new tag, add the tag with space as a separator.
     * For example, if the model tag is preserved, then the value is "-Orientation -Model".
     *
     * @var string
    */
    const PRESERVE_TAGS = "-Orientation";

    /** @var int DEFAULT_JPEG_COMPRESSION Default JPEG compression quality. */
    const DEFAULT_JPEG_COMPRESSION = 90;

    /** @var bool $useexiftool Flag indicating whether to use ExifTool. */
    private bool $useexiftool = false;

    /** @var int Seconds to wait for the shared ExifTool process to finish redacting one file. */
    private const EXIFTOOL_READY_TIMEOUT = 60;

    /** @var resource|null Shared long-lived ExifTool process (-stay_open mode), reused across all files. */
    private static $exiftoolprocess = null;

    /** @var array|null Pipes [0 => stdin, 1 => stdout] of the shared ExifTool process. */
    private static ?array $exiftoolpipes = null;

    /** @var bool Whether the shutdown handler for the shared ExifTool process has been registered. */
    private static bool $exiftoolshutdownregistered = false;

    /** @var bool Set once the shared ExifTool process has failed to start, to stop retrying. */
    private static bool $exiftoolpersistentfailed = false;

    /** @var int Consecutive persistent-process failures; after a few we stop using it. */
    private static int $exiftoolconsecutivefailures = 0;

    /** @var int Normal orientation (no rotation). */
    private const TOP_LEFT = 1;

    /** @var int Mirrored horizontally. */
    private const TOP_RIGHT = 2;

    /** @var int Rotated 180° (upside down). */
    private const BOTTOM_RIGHT = 3;

    /** @var int Mirrored vertically. */
    private const BOTTOM_LEFT = 4;

    /** @var int Mirrored horizontally and rotated 270° clockwise. */
    private const LEFT_TOP = 5;

    /** @var int Rotated 90° clockwise. */
    private const RIGHT_TOP = 6;

    /** @var int Mirrored horizontally and rotated 90° clockwise. */
    private const RIGHT_BOTTOM = 7;

    /** @var int Rotated 270° clockwise. */
    private const LEFT_BOTTOM = 8;

    /**
     * Initialise the EXIF remover service.
     */
    public function __construct() {
        // To decide whether to use ExifTool or PHP GD, check the ExifTool path.
        if (!empty($this->get_exiftool_path())) {
            $this->useexiftool = true;
        }
    }

    #[\Override]
    public function redact_file_by_path(
        string $mimetype,
        string $filepath,
    ): ?string {
        if (!$this->is_mimetype_supported($mimetype)) {
            return null;
        }

        if ($this->useexiftool) {
            // Use the ExifTool executable to remove the desired EXIF tags.
            return $this->execute_exiftool($filepath);
        } else {
            // Use PHP GD lib to remove all EXIF tags.
            return $this->execute_gd($filepath);
        }
    }

    #[\Override]
    public function redact_file_by_content(
        string $mimetype,
        string $filecontent,
    ): ?string {
        if (!$this->is_mimetype_supported($mimetype)) {
            return null;
        }

        if ($this->useexiftool) {
            // Use the ExifTool executable to remove the desired EXIF tags.
            return $this->execute_exiftool_on_content($filecontent);
        } else {
            // Use PHP GD lib to remove all EXIF tags.
            return $this->execute_gd_on_content($filecontent);
        }
    }

    /**
     * Executes ExifTool to remove metadata from the original file.
     *
     * @param string $sourcefile The file path of the file to redact
     * @return string The destination path of the recreated content
     * @throws moodle_exception If the ExifTool process fails or the destination file is not created.
     */
    private function execute_exiftool(string $sourcefile): string {
        $destinationfile = make_request_directory() . '/' . basename($sourcefile);

        // Prefer a single long-lived ExifTool process (-stay_open mode) shared across
        // every file processed in this request. This avoids spawning — and paying the
        // Perl interpreter + module load cost of — a fresh ExifTool process per file,
        // which is a significant cost on image-heavy operations such as course restore.
        if ($this->redact_with_persistent_exiftool($sourcefile, $destinationfile)) {
            return $destinationfile;
        }

        // Fall back to a one-shot invocation if the persistent process is unavailable.
        $command = $this->get_exiftool_command($sourcefile, $destinationfile);

        // Run the command.
        exec($command, $output, $resultcode);

        // If the return code was not zero or the destination file was not successfully created.
        if ($resultcode !== 0 || !file_exists($destinationfile)) {
            throw new moodle_exception(
                errorcode: 'redactor:exifremover:failedprocessexiftool',
                module: 'core_files',
                a: get_class($this),
                debuginfo: implode($output),
            );
        }

        return $destinationfile;
    }

    /**
     * Redacts one file through the shared long-lived ExifTool process (-stay_open mode).
     *
     * On any failure the persistent process is torn down and false is returned, so the
     * caller transparently falls back to a one-shot ExifTool invocation.
     *
     * @param string $sourcefile The file to redact.
     * @param string $destinationfile The path ExifTool should write the redacted copy to.
     * @return bool True if the redacted file was produced; false to trigger the fallback.
     */
    private function redact_with_persistent_exiftool(string $sourcefile, string $destinationfile): bool {
        // The -stay_open argument-file protocol is newline-delimited and is not shell
        // parsed; a path containing a newline would corrupt it, so fall back instead.
        if (preg_match('/[\r\n]/', $sourcefile . $destinationfile)) {
            return false;
        }

        try {
            $pipes = $this->get_persistent_exiftool_pipes();
            if ($pipes === null) {
                return false;
            }

            // One argument per line, terminated by -execute (the -stay_open protocol).
            // No "--" separator before the source: under -stay_open it would make
            // ExifTool treat the trailing "-execute" as a filename rather than the run
            // command. Moodle file-pool paths are absolute, so "--" is not needed.
            $arguments = array_merge(
                $this->get_exiftool_arguments(),
                ['-o', $destinationfile, $sourcefile, '-execute'],
            );
            if (fwrite($pipes[0], implode("\n", $arguments) . "\n") === false) {
                // The process is no longer usable.
                self::note_persistent_exiftool_failure();
                return false;
            }
            fflush($pipes[0]);

            // ExifTool prints "{ready}" on stdout once the -execute has finished.
            if (!self::wait_for_exiftool_ready($pipes[1])) {
                // The process is unresponsive — tear it down and count the failure.
                self::note_persistent_exiftool_failure();
                return false;
            }

            if (file_exists($destinationfile)) {
                self::$exiftoolconsecutivefailures = 0;
                return true;
            }

            // ExifTool finished but produced no file: a per-file problem, not a broken
            // process — fall back for this file only, keeping the process alive.
            return false;
        } catch (\Throwable $e) {
            self::note_persistent_exiftool_failure();
            return false;
        }
    }

    /**
     * Records a failure of the shared ExifTool process: tears it down, and after a few
     * consecutive failures stops using the persistent path for the rest of the request
     * so a misbehaving process cannot stall every file.
     */
    private static function note_persistent_exiftool_failure(): void {
        self::close_persistent_exiftool();
        if (++self::$exiftoolconsecutivefailures >= 3) {
            self::$exiftoolpersistentfailed = true;
        }
    }

    /**
     * Returns the ExifTool arguments that strip metadata, one element per argument.
     *
     * Unlike {@see get_exiftool_command()} these are not shell quoted: the -stay_open
     * protocol passes each line as a literal argument with no shell involved.
     *
     * @return string[]
     */
    private function get_exiftool_arguments(): array {
        // get_remove_tags() returns a shell-quoted token (e.g. -gps*= wrapped in double
        // quotes so the shell does not glob it); strip those quotes for literal use.
        $arguments = [trim($this->get_remove_tags(), '"'), '-tagsfromfile', '@'];
        foreach (preg_split('/\s+/', trim(self::PRESERVE_TAGS)) as $preservetag) {
            if ($preservetag !== '') {
                $arguments[] = $preservetag;
            }
        }
        return $arguments;
    }

    /**
     * Returns the stdin/stdout pipes of the shared ExifTool process, starting it on
     * first use and restarting it if it has exited.
     *
     * @return array|null [0 => stdin, 1 => stdout], or null if it could not be started.
     */
    private function get_persistent_exiftool_pipes(): ?array {
        if (self::$exiftoolpersistentfailed) {
            return null;
        }

        if (is_resource(self::$exiftoolprocess)) {
            $status = proc_get_status(self::$exiftoolprocess);
            if (!empty($status['running'])) {
                return self::$exiftoolpipes;
            }
            // The process has exited — clean up before starting a fresh one.
            self::close_persistent_exiftool();
        }

        $exiftool = $this->get_exiftool_path();
        if (empty($exiftool)) {
            return null;
        }

        // -stay_open keeps ExifTool resident; "-@ -" reads commands from stdin. stderr
        // is discarded, mirroring the "2> /dev/null" of the one-shot command.
        $descriptors = [
            0 => ['pipe', 'r'],
            1 => ['pipe', 'w'],
            2 => ['file', '/dev/null', 'a'],
        ];
        $pipes = [];
        $process = proc_open(
            escapeshellarg($exiftool) . ' -stay_open True -@ -',
            $descriptors,
            $pipes,
        );
        if (!is_resource($process)) {
            // Do not retry a spawn that cannot succeed; fall back for the rest of the request.
            self::$exiftoolpersistentfailed = true;
            return null;
        }

        self::$exiftoolprocess = $process;
        self::$exiftoolpipes = $pipes;

        // Ensure the resident process is shut down cleanly at the end of the request.
        if (!self::$exiftoolshutdownregistered) {
            \core_shutdown_manager::register_function([self::class, 'close_persistent_exiftool']);
            self::$exiftoolshutdownregistered = true;
        }

        return self::$exiftoolpipes;
    }

    /**
     * Blocks until the shared ExifTool process emits its "{ready}" marker for the most
     * recent -execute, or the timeout elapses.
     *
     * @param resource $stdout stdout pipe of the shared process.
     * @return bool True if "{ready}" was seen; false on timeout or EOF.
     */
    private static function wait_for_exiftool_ready($stdout): bool {
        $deadline = microtime(true) + self::EXIFTOOL_READY_TIMEOUT;
        $buffer = '';
        while (microtime(true) < $deadline) {
            $read = [$stdout];
            $write = $except = null;
            $ready = stream_select($read, $write, $except, 1);
            if ($ready === false) {
                return false;
            }
            if ($ready === 0) {
                continue; // Nothing yet — keep waiting until the deadline.
            }
            $chunk = fread($stdout, 8192);
            if ($chunk === false || $chunk === '') {
                return false; // EOF — the process has gone away.
            }
            $buffer .= $chunk;
            if (strpos($buffer, '{ready}') !== false) {
                return true;
            }
        }
        return false;
    }

    /**
     * Shuts down the shared ExifTool process if it is running.
     *
     * Public and static so it can be registered as a request shutdown handler; also
     * called whenever the process is found to have died unexpectedly.
     */
    public static function close_persistent_exiftool(): void {
        if (is_array(self::$exiftoolpipes)) {
            // Asking ExifTool to leave -stay_open mode lets it exit cleanly.
            if (is_resource(self::$exiftoolpipes[0] ?? null)) {
                @fwrite(self::$exiftoolpipes[0], "-stay_open\nFalse\n");
                @fclose(self::$exiftoolpipes[0]);
            }
            if (is_resource(self::$exiftoolpipes[1] ?? null)) {
                @fclose(self::$exiftoolpipes[1]);
            }
        }
        if (is_resource(self::$exiftoolprocess)) {
            @proc_close(self::$exiftoolprocess);
        }
        self::$exiftoolprocess = null;
        self::$exiftoolpipes = null;
    }

    /**
     * Executes ExifTool to remove metadata from the original file content.
     *
     * @param string $filecontent The file content to redact.
     * @return string The redacted updated content
     * @throws moodle_exception If the ExifTool process fails or the destination file is not created.
     */
    private function execute_exiftool_on_content(string $filecontent): string {
        $sourcefile = make_request_directory() . '/input';
        file_put_contents($sourcefile, $filecontent);

        $destinationfile = $this->execute_exiftool($sourcefile);
        return file_get_contents($destinationfile);
    }

    /**
     * Executes GD library to remove metadata from the original file.
     *
     * @param string $sourcefile The source file to redact.
     * @return string The destination path of the recreated content
     * @throws moodle_exception If the image data is not successfully recreated.
     */
    private function execute_gd(string $sourcefile): string {
        // Read EXIF data from the temporary file.
        $exifdata = @exif_read_data($sourcefile);
        $orientation = isset($exifdata['Orientation']) ? $exifdata['Orientation'] : self::TOP_LEFT;

        $filecontent = file_get_contents($sourcefile);
        $destinationfile = $this->recreate_image_gd($filecontent, $orientation);
        if (!$destinationfile) {
            throw new moodle_exception(
                errorcode: 'redactor:exifremover:failedprocessgd',
                module: 'core_files',
                a: get_class($this),
            );
        }

        return $destinationfile;
    }

    /**
     * Executes GD library to remove metadata from the original file.
     *
     * @param string $filecontent The source file content to redact.
     * @return string The redacted file content
     * @throws moodle_exception If the image data is not successfully recreated.
     */
    private function execute_gd_on_content(string $filecontent): string {
        $destinationfile = $this->recreate_image_gd($filecontent);
        if (!$destinationfile) {
            throw new moodle_exception(
                errorcode: 'redactor:exifremover:failedprocessgd',
                module: 'core_files',
                a: get_class($this),
            );
        }

        return file_get_contents($destinationfile);
    }

    /**
     * Gets the ExifTool command to strip the file of EXIF data.
     *
     * @param string $source The source path of the file.
     * @param string $destination The destination path of the file.
     * @return string The command to use to remove EXIF data from the file.
     */
    private function get_exiftool_command(string $source, string $destination): string {
        $exiftoolexec = escapeshellarg($this->get_exiftool_path());
        $removetags = $this->get_remove_tags();
        $tempdestination = escapeshellarg($destination);
        $tempsource = escapeshellarg($source);
        $preservetagsoption = "-tagsfromfile @ " . self::PRESERVE_TAGS;
        $command = "$exiftoolexec $removetags $preservetagsoption -o $tempdestination -- $tempsource";
        $command .= " 2> /dev/null"; // Do not output any errors.
        return $command;
    }

    /**
     * Retrieves the remove tag options based on configuration.
     *
     * @return string The remove tag options.
     */
    private function get_remove_tags(): string {
        $removetags = get_config('core', 'file_redactor_exifremoverremovetags');
        // If the remove tags value is empty or not empty but does not exist in the array, then set the default.
        if (!$removetags || ($removetags && !array_key_exists($removetags, self::REMOVE_TAGS))) {
            $removetags = self::DEFAULT_REMOVE_TAGS;
        }
        return self::REMOVE_TAGS[$removetags];
    }

    /**
     * Retrieves the path to the ExifTool executable.
     *
     * @return string The path to the ExifTool executable.
     */
    private function get_exiftool_path(): string {
        $toolpathconfig = get_config('core', 'file_redactor_exifremovertoolpath');
        if (!empty($toolpathconfig) && is_executable($toolpathconfig)) {
            return $toolpathconfig;
        }
        return '';
    }

    /**
     * Recreate the image using PHP GD library to strip all EXIF data.
     *
     * @param string $content The source file content.
     * @param int $orientation The orientation value. The default is 1, which means no rotation.
     * @return null|string The path to the recreated image, or null on failure.
     */
    private function recreate_image_gd(
        string $content,
        int $orientation = self::TOP_LEFT,
    ): ?string {
        // Fetch the image information for this image.
        $imageinfo = @getimagesizefromstring($content);
        if (empty($imageinfo)) {
            return null;
        }
        // Create a new image from the file.
        $image = @imagecreatefromstring($content);

        $this->flip_gd($image, $orientation);

        $destinationfile = make_request_directory() . '/output';

        // Capture the image as a string object, rather than straight to file.
        $result = imagejpeg(
            image: $image,
            file: $destinationfile,
            quality: self::DEFAULT_JPEG_COMPRESSION,
        );

        imagedestroy($image);

        if ($result) {
            return $destinationfile;
        }

        return null;
    }

    /**
     * Flips the given GD image resource based on the specified orientation.
     *
     * @param \GDImage $image The GD image resource to be flipped.
     * @param int $orientation The orientation value indicating how the image should be flipped.
     *
     * @return void
     */
    private function flip_gd(\GDImage &$image, int $orientation): void {
        switch ($orientation) {
            case self::TOP_LEFT:
                break;
            case self::TOP_RIGHT:
                imageflip($image, IMG_FLIP_HORIZONTAL);
                break;
            case self::BOTTOM_RIGHT:
                $image = imagerotate($image, 180, 0);
                break;
            case self::BOTTOM_LEFT:
                imageflip($image, IMG_FLIP_VERTICAL);
                break;
            case self::LEFT_TOP:
                $image = imagerotate($image, -90, 0);
                imageflip($image, IMG_FLIP_HORIZONTAL);
                break;
            case self::RIGHT_TOP:
                $image = imagerotate($image, -90, 0);
                break;
            case self::RIGHT_BOTTOM:
                $image = imagerotate($image, 90, 0);
                imageflip($image, IMG_FLIP_HORIZONTAL);
                break;
            case self::LEFT_BOTTOM:
                $image = imagerotate($image, 90, 0);
                break;
        }
    }

    /**
     * Returns true if the service is enabled, and false if it is not.
     *
     * @return bool
     */
    public function is_enabled(): bool {
        return (bool) get_config('core', 'file_redactor_exifremoverenabled');
    }

    /**
     * Determines whether a certain mime-type is supported by the service.
     * It will return true if the mime-type is supported, and false if it is not.
     *
     * @param string $mimetype The mime type of file.
     * @return bool
     */
    public function is_mimetype_supported(string $mimetype): bool {
        if ($mimetype === self::DEFAULT_MIMETYPE) {
            return true;
        }

        if ($this->useexiftool) {
            // Get the supported MIME types from the config if using ExifTool.
            $supportedmimetypesconfig = get_config('core', 'file_redactor_exifremovermimetype');
            $supportedmimetypes = array_filter(array_map('trim', explode("\n",  $supportedmimetypesconfig)));
            return in_array($mimetype, $supportedmimetypes) ?? false;
        }

        return false;
    }

    /**
     * Adds settings to the provided admin settings page.
     *
     * @param \admin_settingpage $settings The admin settings page to which settings are added.
     */
    public static function add_settings(\admin_settingpage $settings): void {
        global $OUTPUT;

        // Enabled for a fresh install, disabled for an upgrade.
        $defaultenabled = 1;

        if (empty(get_config('core', 'file_redactor_exifremoverenabled'))) {
            if (PHPUNIT_TEST || !during_initial_install()) {
                $defaultenabled = 0;
            }
        }

        $icon = $OUTPUT->pix_icon('i/externallink', get_string('opensinnewwindow'));
        $a = (object) [
            'link' => html_writer::link(
                url: 'https://exiftool.sourceforge.net/install.html',
                text: "https://exiftool.sourceforge.net/install.html $icon",
                attributes: ['role' => 'opener', 'rel' => 'noreferrer', 'target' => '_blank'],
            ),
        ];

        $settings->add(
            new admin_setting_configcheckbox(
                name: 'file_redactor_exifremoverenabled',
                visiblename: get_string('redactor:exifremover:enabled', 'core_files'),
                description: get_string('redactor:exifremover:enabled_desc', 'core_files', $a),
                defaultsetting: $defaultenabled,
            ),
        );

        $settings->add(
            new admin_setting_heading(
                name: 'exifremoverheading',
                heading: get_string('redactor:exifremover:heading', 'core_files'),
                information: '',
            )
        );

        $settings->add(
            new admin_setting_configexecutable(
                name: 'file_redactor_exifremovertoolpath',
                visiblename: get_string('redactor:exifremover:toolpath', 'core_files'),
                description: get_string('redactor:exifremover:toolpath_desc', 'core_files'),
                defaultdirectory: '',
            )
        );

        foreach (array_keys(self::REMOVE_TAGS) as $key) {
            $removedtagchoices[$key] = get_string("redactor:exifremover:tag:$key", 'core_files');
        }
        $settings->add(
            new admin_setting_configselect(
                name: 'file_redactor_exifremoverremovetags',
                visiblename: get_string('redactor:exifremover:removetags', 'core_files'),
                description: get_string('redactor:exifremover:removetags_desc', 'core_files'),
                defaultsetting: self::DEFAULT_REMOVE_TAGS,
                choices: $removedtagchoices,
            ),
        );

        $mimetypedefault = <<<EOF
        image/jpeg
        image/tiff
        EOF;
        $settings->add(
            new admin_setting_configtextarea(
                name: 'file_redactor_exifremovermimetype',
                visiblename: get_string('redactor:exifremover:mimetype', 'core_files'),
                description: get_string('redactor:exifremover:mimetype_desc', 'core_files'),
                defaultsetting: $mimetypedefault,
            ),
        );
    }
}
