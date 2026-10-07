-- ************************************************************
-- Title:	PUBLIC_DVD_P5MINSCENARIODEMANDTRK_202106.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_SCENARIODEMANDTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_P5MINSCENARIODEMANDTRK_202106010000.CSV
APPEND INTO TABLE P5MIN_SCENARIODEMANDTRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSION_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
