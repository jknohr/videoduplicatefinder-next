// /*
//     Copyright (C) 2025 0x90d
//     This file is part of VideoDuplicateFinder
//     VideoDuplicateFinder is free software: you can redistribute it and/or modify
//     it under the terms of the GNU Affero General Public License as published by
//     the Free Software Foundation, either version 3 of the License, or
//     (at your option) any later version.
//     VideoDuplicateFinder is distributed in the hope that it will be useful,
//     but WITHOUT ANY WARRANTY without even the implied warranty of
//     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//     GNU Affero General Public License for more details.
//     You should have received a copy of the GNU Affero General Public License
//     along with VideoDuplicateFinder.  If not, see <http://www.gnu.org/licenses/>.
// */
//

namespace VDF.Web.Utils {
	internal static class MimeTypeHelper {
		internal static string GetVideoMimeType(string extension) =>
			extension.TrimStart('.').ToLowerInvariant() switch {
				"mp4"  => "video/mp4",
				"m4v"  => "video/mp4",
				"mkv"  => "video/x-matroska",
				"webm" => "video/webm",
				"avi"  => "video/x-msvideo",
				"mov"  => "video/quicktime",
				"wmv"  => "video/x-ms-wmv",
				"flv"  => "video/x-flv",
				"ts"   => "video/mp2t",
				"m2ts" => "video/mp2t",
				"mpg"  => "video/mpeg",
				"mpeg" => "video/mpeg",
				"3gp"  => "video/3gpp",
				"ogv"  => "video/ogg",
				_      => "application/octet-stream"
			};
	}
}
