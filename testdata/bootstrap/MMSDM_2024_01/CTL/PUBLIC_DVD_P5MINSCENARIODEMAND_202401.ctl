-- ************************************************************
-- Title:	PUBLIC_DVD_P5MINSCENARIODEMAND_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS P5MIN_SCENARIODEMAND owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_P5MINSCENARIODEMAND_202401010000.CSV
APPEND INTO TABLE P5MIN_SCENARIODEMAND
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
SCENARIO FLOAT EXTERNAL,
REGIONID CHAR(20),
DELTAMW FLOAT EXTERNAL)
-- End of Script
