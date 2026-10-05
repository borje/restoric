--- @sync entry
--- restoric.yazi: browse the hovered folder's history with restoric.
--- A hovered file opens its folder with the file selected (restoric does
--- that for a file path). Quitting restoric (q) comes back to the same
--- place in yazi.
---
--- It runs restoric through yazi's own `shell --block`, which hides yazi
--- while restoric has the terminal.

--- A shell word, with `%` escaped for yazi's command template.
local function word(s)
	return (ya.quote(s):gsub("%%", "%%%%"))
end

return {
	entry = function(_, job)
		local folder = cx.active.current
		local h = folder.hovered
		local target = h and tostring(h.url) or tostring(folder.cwd)
		local cmd = "restoric " .. word(target)

		-- Arguments after `--` go to restoric, e.g. `plugin restoric -- --no-icons`.
		-- yazi parses `--name` and `--name=value` itself (and turns `-` in
		-- names into `_`), so put them back.
		for k, v in pairs(job.args or {}) do
			if type(k) == "number" then
				cmd = cmd .. " " .. word(tostring(v))
			else
				local name = "--" .. k:gsub("_", "-")
				cmd = cmd .. " " .. word(v == true and name or name .. "=" .. tostring(v))
			end
		end

		ya.emit("shell", { cmd, block = true })
	end,
}
